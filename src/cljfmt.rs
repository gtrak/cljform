//! cljfmt format regime (issue 43) — native port of cljfmt's default-rule
//! whitespace semantics.
//!
//! Attribution (verified before landing):
//!   * cljfmt — https://github.com/weavejester/cljfmt — Eclipse Public License
//!     1.0 (LICENSE.txt upstream). Pinned: tag 0.16.6 @ commit
//!     baab5008032945434cbca23ef5eda516e3ea97b0.
//!     NOTE: the task file's URL "weavesjesser/cljfmt" 404s (single-s
//!     `weavejester` is correct) — verified live against the upstream repo
//!     before landing this port.
//!   * Vendored data (same pin): cljfmt/resources/cljfmt/indents/clojure.clj,
//!     indents/compojure.clj, indents/fuzzy.clj (default indents table).
//!   * rewrite-clj v1.2.50 (clj-commons/rewrite-clj, Apache-2.0) — node model
//!     and zipper semantics ported structurally (zipper paths, sibling skips,
//!     depth-first next/prev, margin computation).
//!
//! Pipeline (cljfmt 0.16.6 `reformat-form` with default options, applied to
//! the whole file — the file's own `#_{:cljfmt ...}` metadata is NOT honored;
//! that is the documented divergence from cljfmt's config-file mode):
//!   1. parse (rewrite-clj `parse-string-all` semantics; one node per token)
//!   2. remove-consecutive-blank-lines
//!   3. remove-surrounding-whitespace
//!   4. insert-missing-whitespace
//!   5. unindent, then indent (default indents table below)
//!   6. remove-trailing-whitespace
//!
//! The candidate is produced from CRLF-normalized text; the file's original
//! line separator (the first `\r\n`/`\n` seen) is restored on output
//! (cljfmt behavior: `\n` in, `\n` out; `\r\n` in, `\r\n` out; mixed →
//! first separator wins).
//!
//! Differential verification against live cljfmt 0.16.6: tests/cljfmt-diff/
//! (corpus + byte-equal snapshots + idempotence + parinfer-regime baseline).
//! Live-reference probes (run at implementation time against the pinned
//! cljfmt checkout) are documented in tests/cljfmt-diff/probes.md.

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum FmtRegime {
    /// Native port of cljfmt 0.16.6's default-rule whitespace semantics.
    Cljfmt,
    /// The existing parinfer paren-mode reindent (the default).
    Parinfer,
}

#[derive(Debug, Clone)]
pub struct CljfmtError {
    pub line: usize,
    pub col: usize,
    pub message: String,
}

impl CljfmtError {
    fn at(line: usize, col: usize, message: impl Into<String>) -> Self {
        CljfmtError { line, col, message: message.into() }
    }
}

impl std::fmt::Display for CljfmtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (line {} col {})", self.message, self.line, self.col)
    }
}

// ---------------------------------------------------------------------------
// Nodes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    Forms,
    List,
    Vector,
    Map,
    Set,
    Fn,
    Token,
    String,
    Regex,
    Comment,
    Ws,
    Newline,
    Comma,
    RawMeta,
    Var,
    Eval,
    Uneval,
    Quote,
    SyntaxQuote,
    Unquote,
    UnquoteSplicing,
    Deref,
    ReaderMacro,
    NamespacedMap,
    MapQualifier,
}

pub struct Node {
    pub tag: Tag,
    /// Leaves: the node's exact source text. Inner nodes: the opening
    /// delimiter text. Forms: "".
    pub text: String,
    pub children: Vec<Node>,
    // Lazily computed, invalidated on any edit (Cell: computed through &self).
    width: std::cell::Cell<Option<usize>>,
    after_line: std::cell::Cell<Option<Option<usize>>>,
}

impl Node {
    fn leaf(tag: Tag, text: String) -> Node {
        Node { tag, text, children: vec![], width: std::cell::Cell::new(None), after_line: std::cell::Cell::new(None) }
    }
    fn inner(tag: Tag, text: String, children: Vec<Node>) -> Node {
        Node { tag, text, children, width: std::cell::Cell::new(None), after_line: std::cell::Cell::new(None) }
    }
    fn forms(children: Vec<Node>) -> Node {
        Node {
            tag: Tag::Forms,
            text: String::new(),
            children,
            width: std::cell::Cell::new(None),
            after_line: std::cell::Cell::new(None),
        }
    }


    fn full_text(&self) -> String {
        let mut out = String::with_capacity(self.text.len() + 8);
        out.push_str(&self.text);
        for c in &self.children {
            out.push_str(&c.full_text());
        }
        out.push_str(delimiter_for(self.tag));
        out
    }

    fn width(&self) -> usize {
        match self.width.get() {
            Some(w) => w,
            None => {
                let w = utf16_width(&self.full_text());
                self.width.set(Some(w));
                w
            }
        }
    }

    /// `None` → this node's full text contains no `\n`. `Some(w)` → utf16
    /// width of the text after the last `\n` in this node.
    fn after_line(&self) -> Option<usize> {
        match self.after_line.get() {
            Some(v) => v,
            None => {
                let v = if self.children.is_empty() {
                    self.text.rfind('\n').map(|p| utf16_width(&self.text[p + 1..]))
                } else {
                    let mut acc = None;
                    for c in &self.children {
                        acc = match acc {
                            None => c.after_line(),
                            Some(w) => Some(w + c.width()),
                        };
                    }
                    // The closing delimiter follows every child, so it counts
                    // once a newline has been found in the subtree.
                    acc.map(|w| w + utf16_width(delimiter_for(self.tag)))
                };
                self.after_line.set(Some(v));
                v
            }
        }
    }
}

fn utf16_width(s: &str) -> usize {
    s.encode_utf16().count()
}

fn delimiter_for(tag: Tag) -> &'static str {
    match tag {
        Tag::List | Tag::Fn => ")",
        Tag::Vector => "]",
        Tag::Map | Tag::Set => "}",
        _ => "",
    }
}

fn is_ws_or_comment(t: Tag) -> bool {
    matches!(t, Tag::Ws | Tag::Newline | Tag::Comma | Tag::Comment)
}
fn is_clojure_ws(t: Tag) -> bool {
    matches!(t, Tag::Ws | Tag::Newline | Tag::Comma)
}
fn is_comment(t: Tag) -> bool {
    t == Tag::Comment
}
fn is_element(t: Tag) -> bool {
    !is_ws_or_comment(t)
}
fn is_uneval(t: Tag) -> bool {
    t == Tag::Uneval
}
fn is_meta(t: Tag) -> bool {
    t == Tag::RawMeta
}

fn invalidate_ancestors(root: &mut Node, path: &[usize]) {
    for k in 0..=path.len() {
        let mut cur = &mut *root;
        for &i in &path[..k] {
            cur = &mut cur.children[i];
        }
        cur.width.set(None);
        cur.after_line.set(None);
    }
}

// ---------------------------------------------------------------------------
// Parser (rewrite-clj 1.2.50 semantics for the default reader features)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokVal {
    Sym,
    Num,
    Kw,
    Other,
}

struct P<'a> {
    s: &'a str,
    i: usize,
    line: usize,
    col: usize,
}

impl<'a> P<'a> {
    fn new(s: &'a str) -> Self {
        P { s, i: 0, line: 1, col: 1 }
    }
    fn peek(&self) -> Option<char> {
        self.s.get(self.i..).and_then(|r| r.chars().next())
    }
    fn ch(&mut self) -> Option<char> {
        let c = self.peek()?;
        let w = c.len_utf8();
        self.i += w;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }
    fn err(&self, m: impl Into<String>) -> CljfmtError {
        CljfmtError::at(self.line, self.col, m)
    }
    fn read_while(&mut self, f: impl Fn(char) -> bool) -> String {
        let mut out = String::new();
        while let Some(c) = self.peek() {
            if !f(c) {
                break;
            }
            out.push(c);
            self.ch();
        }
        out
    }
}

fn clojure_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | ',')
}
fn is_boundary(c: char) -> bool {
    matches!(c, '(' | ')' | '[' | ']' | '{' | '}' | '"')
}
fn ws_or_boundary(c: char) -> bool {
    clojure_ws(c) || is_boundary(c)
}

enum Item {
    Node(Node),
    Close,
    Eof,
}

impl<'a> P<'a> {
    fn dispatch(&mut self, delim: Option<char>) -> Result<Item, CljfmtError> {
        let c = match self.peek() {
            Some(c) => c,
            None => return Ok(Item::Eof),
        };
        if Some(c) == delim {
            self.ch();
            return Ok(Item::Close);
        }
        // A stray closing delimiter (outside the matching collection) is a
        // reader error. Opening delimiters dispatch normally below.
        if matches!(c, ')' | ']' | '}') && Some(c) != delim {
            return Err(self.err(format!("Unmatched delimiter: {c}")));
        }
        match c {
            '(' => {
                self.ch();
                Ok(Item::Node(Node::inner(
                    Tag::List,
                    "(".into(),
                    self.parse_until(')')?,
                )))
            }
            '[' => {
                self.ch();
                Ok(Item::Node(Node::inner(
                    Tag::Vector,
                    "[".into(),
                    self.parse_until(']')?,
                )))
            }
            '{' => {
                self.ch();
                Ok(Item::Node(Node::inner(
                    Tag::Map,
                    "{".into(),
                    self.parse_until('}')?,
                )))
            }
            ';' => {
                self.ch();
                let body = self.read_while(|c| !matches!(c, '\n' | '\r'));
                let mut text = format!(";{body}");
                if matches!(self.peek(), Some('\n') | Some('\r')) {
                    text.push(self.ch().unwrap());
                }
                Ok(Item::Node(Node::leaf(Tag::Comment, text)))
            }
            ' ' | '\t' => Ok(Item::Node(Node::leaf(
                Tag::Ws,
                self.read_while(|c| c == ' ' || c == '\t'),
            ))),
            '\n' | '\r' => Ok(Item::Node(Node::leaf(
                Tag::Newline,
                self.read_while(|c| matches!(c, '\n' | '\r')),
            ))),
            ',' => Ok(Item::Node(Node::leaf(Tag::Comma, self.read_while(|c| c == ',')))),
            '"' => Ok(Item::Node(self.parse_string_data(Tag::String)?)),
            '#' => {
                // rewrite-clj token reader special-cases ##Inf / ##Nan as tokens
                if self.s[self.i..].starts_with("##Inf") || self.s[self.i..].starts_with("##Nan") {
                    let lit = if self.s[self.i..].starts_with("##Inf") { "##Inf" } else { "##Nan" };
                    self.i += lit.len();
                    return Ok(Item::Node(Node::leaf(Tag::Token, lit.to_string())));
                }
                Ok(Item::Node(self.parse_sharp()?))
            }
            '~' => {
                self.ch();
                if self.peek() == Some('@') {
                    self.ch();
                    Ok(Item::Node(Node::inner(
                        Tag::UnquoteSplicing,
                        "~@".into(),
                        self.parse_printables("unquote-splicing", 1)?,
                    )))
                } else {
                    Ok(Item::Node(Node::inner(
                        Tag::Unquote,
                        "~".into(),
                        self.parse_printables("unquote", 1)?,
                    )))
                }
            }
            '\'' => {
                self.ch();
                Ok(Item::Node(Node::inner(
                    Tag::Quote,
                    "'".into(),
                    self.parse_printables("quote", 1)?,
                )))
            }
            '`' => {
                self.ch();
                Ok(Item::Node(Node::inner(
                    Tag::SyntaxQuote,
                    "`".into(),
                    self.parse_printables("syntax-quote", 1)?,
                )))
            }
            '@' => {
                self.ch();
                Ok(Item::Node(Node::inner(
                    Tag::Deref,
                    "@".into(),
                    self.parse_printables("deref", 1)?,
                )))
            }
            ':' => {
                self.ch();
                let mut text = String::from(":");
                if self.peek() == Some(':') {
                    text.push(':');
                    self.ch();
                }
                let body = self.read_while(|c| !ws_or_boundary(c));
                if body.is_empty() {
                    // `::` alone is the valid auto-resolved nil keyword
                    if text == "::" && self.peek().is_some_and(is_boundary) {
                        return Ok(Item::Node(Node::leaf(Tag::Token, text)));
                    }
                    let msg = if self.peek().is_none() {
                        "unexpected EOF while reading keyword."
                    } else {
                        "Invalid keyword: "
                    };
                    return Err(self.err(msg));
                }
                text.push_str(&body);
                Ok(Item::Node(Node::leaf(Tag::Token, text)))
            }
            _ => Ok(Item::Node(self.parse_token()?)),
        }
    }

    fn parse_until(&mut self, delim: char) -> Result<Vec<Node>, CljfmtError> {
        let mut kids = vec![];
        loop {
            match self.dispatch(Some(delim))? {
                Item::Node(n) => kids.push(n),
                Item::Close => return Ok(kids),
                Item::Eof => return Err(self.err("Unexpected EOF.")),
            }
        }
    }

    fn parse_printables(&mut self, tag: &str, n: u32) -> Result<Vec<Node>, CljfmtError> {
        let mut kids: Vec<Node> = vec![];
        let mut count: u32 = 0;
        while count < n {
            match self.dispatch(None)? {
                Item::Node(x) => {
                    if is_element(x.tag) {
                        count += 1;
                    }
                    kids.push(x);
                }
                Item::Close => return Err(self.err("Unexpected close.")),
                Item::Eof => {
                    return Err(self.err(format!(
                        "{tag} node expects {n} value{}.",
                        if n == 1 { "" } else { "s" }
                    )));
                }
            }
        }
        Ok(kids)
    }

    fn parse_string_data(&mut self, tag: Tag) -> Result<Node, CljfmtError> {
        let start = self.i;
        self.ch(); // opening quote
        let mut escape = false;
        loop {
            match self.ch() {
                Some('"') if !escape => break,
                Some(c) => {
                    let was = escape;
                    escape = !was && c == '\\';
                }
                None => {
                    return Err(self.err("Unexpected EOF while reading string."));
                }
            }
        }
        Ok(Node::leaf(tag, self.s[start..self.i].to_string()))
    }

    fn parse_sharp(&mut self) -> Result<Node, CljfmtError> {
        self.ch(); // '#'
        let c2 = self
            .peek()
            .ok_or_else(|| self.err("unexpected EOF while reading dispatch macro"))?;
        match c2 {
            ':' => {
                self.ch();
                let (qual, ws) = self.parse_namespaced_map_qualifier()?;
                let mut kids = vec![qual];
                kids.extend(ws);
                match self.dispatch(None)? {
                    Item::Node(m) if m.tag == Tag::Map => kids.push(m),
                    Item::Node(_) => return Err(self.err("namespaced map expects a map")),
                    Item::Close => return Err(self.err("Unexpected close.")),
                    Item::Eof => return Err(self.err("Unexpected EOF.")),
                }
                Ok(Node::inner(Tag::NamespacedMap, "#".into(), kids))
            }
            '?' => {
                self.ch(); // '?'
                let at = self.peek() == Some('@');
                if at {
                    self.ch();
                }
                let mut kids = vec![Node::leaf(Tag::Token, if at { "?@" } else { "?" }.to_string())];
                match self.dispatch(None)? {
                    Item::Node(m) => kids.push(m),
                    Item::Close => return Err(self.err("Unexpected close.")),
                    Item::Eof => return Err(self.err("Unexpected EOF.")),
                }
                Ok(Node::inner(Tag::ReaderMacro, "#".into(), kids))
            }
            '{' => {
                self.ch();
                Ok(Node::inner(Tag::Set, "#{".into(), self.parse_until('}')?))
            }
            '(' => {
                self.ch();
                Ok(Node::inner(Tag::Fn, "#(".into(), self.parse_until(')')?))
            }
            '"' => {
                let start = self.i;
                self.parse_string_data(Tag::String)?;
                // rebuild the regex node over the full source span (#"..."")
                Ok(Node::leaf(Tag::Regex, self.s[start - 1..self.i].to_string()))
            }
            '^' => {
                self.ch();
                Ok(Node::inner(
                    Tag::RawMeta,
                    "#^".into(),
                    self.parse_printables("meta*", 2)?,
                ))
            }
            '\'' => {
                self.ch();
                Ok(Node::inner(
                    Tag::Var,
                    "#'".into(),
                    self.parse_printables("var", 1)?,
                ))
            }
            '=' => {
                self.ch();
                Ok(Node::inner(
                    Tag::Eval,
                    "#=".into(),
                    self.parse_printables("eval", 1)?,
                ))
            }
            '_' => {
                self.ch();
                Ok(Node::inner(
                    Tag::Uneval,
                    "#_".into(),
                    self.parse_printables("uneval", 1)?,
                ))
            }
            _ => Ok(Node::inner(
                Tag::ReaderMacro,
                "#".into(),
                self.parse_printables("reader-macro", 2)?,
            )),
        }
    }

    fn parse_namespaced_map_qualifier(&mut self) -> Result<(Node, Vec<Node>), CljfmtError> {
        let auto = if self.peek() == Some(':') {
            self.ch();
            true
        } else {
            false
        };
        let prefix = self.read_while(|c| !is_boundary(c) && !clojure_ws(c));
        if !auto && prefix.is_empty() {
            return Err(self.err("namespaced map expects a namespace"));
        }
        let qual = if auto {
            Node::leaf(Tag::MapQualifier, format!("::{prefix}"))
        } else {
            Node::leaf(Tag::MapQualifier, format!(":{prefix}"))
        };
        // Consume only the whitespace between the qualifier and the map;
        // the map itself is left for the `#:` dispatch arm to read.
        let mut ws: Vec<Node> = vec![];
        while self.peek().is_some_and(clojure_ws) {
            match self.dispatch(None)? {
                Item::Node(n) if is_clojure_ws(n.tag) => ws.push(n),
                _ => return Err(self.err("namespaced map expects a map")),
            }
        }
        Ok((qual, ws))
    }

    fn parse_token(&mut self) -> Result<Node, CljfmtError> {
        let first = self.ch().ok_or_else(|| self.err("unexpected EOF while reading token."))?;
        if first == '\\' {
            return self.parse_char_literal();
        }
        let body = self.read_while(|c| !ws_or_boundary(c));
        let mut s = first.to_string();
        s.push_str(&body);
        // classify: invalid numbers/symbols are reader errors (rewrite-clj)
        classify_token(&s).map_err(|m| self.err(m))?;
        Ok(Node::leaf(Tag::Token, s))
    }

    fn parse_char_literal(&mut self) -> Result<Node, CljfmtError> {
        // Mirror rewrite-clj `read-to-char-boundary`: the char after `\\` is
        // consumed unconditionally; if it is `\\`, the literal is exactly
        // `\\\\`; otherwise the name runs to the next whitespace/boundary.
        // `\\` was already consumed by the token dispatch; `ch` consumes
        // the char after it.
        let c = self.ch().ok_or_else(|| self.err("EOF while reading character."))?;
        if c == '\\' {
            return Ok(Node::leaf(Tag::Token, "\\\\".to_string()));
        }
        let name = self.read_while(|ch| !charlit_boundary(ch));
        let full = format!("\\{c}{name}");
        // tools.reader: an escape word or `\\u`+4 hex, or a single char;
        // trailing commas are ignored by the reader but still lexed in.
        let core = match name.find(',') {
            None => name.as_str(),
            Some(i) => {
                if name[i + 1..].chars().any(|ch| ch != ',') {
                    return Err(self.err(format!("Unsupported character: {full}.")));
                }
                &name[..i]
            }
        };
        let ok = match c {
            'u' => core.len() == 4 && core.bytes().all(|b| b.is_ascii_hexdigit()),
            _ => core.is_empty()
                || matches!(
                    format!("\\{c}{core}").as_str(),
                    "\\newline" | "\\space" | "\\tab" | "\\backspace" | "\\formfeed" | "\\return"
                ),
        };
        if !ok {
            return Err(self.err(format!("Unsupported character: {full}.")));
        }
        Ok(Node::leaf(Tag::Token, full))
    }

}

/// rewrite-clj `whitespace-or-boundary?` — the stop set for char-literal
/// names (reader.cljc `boundary?` + whitespace).
fn charlit_boundary(c: char) -> bool {
    clojure_ws(c)
        || matches!(
            c,
            '"' | ':' | ';' | '\'' | '@' | '^' | '`' | '~' | '(' | ')' | '[' | ']' | '{' | '}' | '\\'
        )
}

fn is_decimal_int(s: &str) -> bool {
    let b = s.as_bytes();
    let rest = if matches!(b.first(), Some(b'-') | Some(b'+')) {
        &b[1..]
    } else {
        b
    };
    !rest.is_empty() && rest.iter().all(|c| c.is_ascii_digit())
}

fn validate_number(s: &str) -> bool {
    if s.contains('/') {
        let mut parts = s.splitn(2, '/');
        match (parts.next(), parts.next()) {
            (Some(a), Some(b)) => is_decimal_int(a) && is_decimal_int(b),
            _ => false,
        }
    } else if let Some(x) = s.strip_prefix("0x").or_else(|| s.strip_prefix("+0x")).or_else(|| s.strip_prefix("-0x"))
    {
        !x.is_empty() && x.bytes().all(|c| c.is_ascii_hexdigit())
    } else if let Some(rest) = s.strip_prefix('0') {
        // octal (leading zero) or decimal starting with 0
        if rest.is_empty() {
            true
        } else {
            is_decimal_int(s)
        }
    } else {
        let digits = |v: &[u8]| !v.is_empty() && v.iter().all(|c| c.is_ascii_digit());
        let has_exp = s.rfind(['e', 'E']);
        let body = match has_exp {
            Some(p) => &s[..p],
            None => s,
        };
        let exp_ok = has_exp.is_some_and(|p| {
            let e = &s[p + 1..];
            let d = e.strip_prefix(['-', '+']).unwrap_or(e);
            !e.is_empty() && digits(d.as_bytes())
        });
        let body_ok = if body.starts_with('.') {
            digits(&body.as_bytes()[1..])
        } else if body.contains('.') {
            let (i, f) = body.split_once('.').unwrap();
            digits(i.as_bytes()) && (f.is_empty() || digits(f.as_bytes()))
        } else {
            digits(body.as_bytes())
        };
        body_ok && (has_exp.is_none() || exp_ok) && s.contains(['.', 'e', 'E']) || is_decimal_int(s)
    }
}

/// Token classification mirroring rewrite-clj 1.2.50's parse-token:
/// number-literal tokens go through `string->edn` (a full EDN number read);
/// everything else goes through the merged `parse-symbol` (TRDR-73 patch:
/// any slash-less token is a valid symbol; slashed tokens follow the
/// ns/name rules incl. the `foobar/3` array-class exception).
fn classify_token(s: &str) -> Result<TokVal, String> {
    if matches!(s, "nil" | "true" | "false" | "/") {
        return Ok(TokVal::Other);
    }
    // Keywords are their own literal kind — never symbols.
    if s.starts_with(':') {
        return Ok(TokVal::Kw);
    }
    let first = s.as_bytes()[0];
    let second = s.as_bytes().get(1).copied();
    let number_literal = first.is_ascii_digit()
        || (matches!(first, b'+' | b'-') && second.is_some_and(|c| c.is_ascii_digit()));
    if number_literal {
        if !validate_number(s) {
            return Err(format!("Invalid number: {s}"));
        }
        return Ok(TokVal::Num);
    }
    // parse-symbol (rewrite-clj 1.2.50 merged clj/cljs implementation)
    if s.is_empty() || s.ends_with(':') || s.starts_with("::") {
        return Err(format!("Invalid symbol: {s}"));
    }
    if let Some(ns_idx) = s.find('/') {
        let ns = &s[..ns_idx];
        let sym = &s[ns_idx + 1..];
        let valid = if sym.is_empty() {
            false
        } else if matches!(sym, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9") {
            true
        } else {
            !sym.as_bytes()[0].is_ascii_digit()
                && !ns.ends_with(':')
                && (sym == "/" || !sym.contains('/'))
        };
        if !valid {
            return Err(format!("Invalid symbol: {s}"));
        }
    }
    Ok(TokVal::Sym)
}

/// Parse the full text into a Forms node.
pub fn parse_forms(text: &str) -> Result<Node, CljfmtError> {
    let mut p = P::new(text);
    let mut kids = vec![];
    loop {
        match p.dispatch(None)? {
            Item::Node(n) => kids.push(n),
            Item::Close => {
                let e = CljfmtError::at(p.line, p.col, "Unmatched delimiter.");
                return Err(e);
            }
            Item::Eof => break,
        }
    }
    Ok(Node::forms(kids))
}

// ---------------------------------------------------------------------------
// Main entry
// ---------------------------------------------------------------------------

pub fn format_cljfmt(text: &str) -> Result<String, CljfmtError> {
    let sep = line_separator(text);
    // cljfmt normalize-newlines: \r\n -> \n (stray \r is left alone)
    let normalized = text.replace("\r\n", "\n");
    let mut root = parse_forms(&normalized)?;
    pass_consecutive_blank_lines(&mut root);
    pass_remove_surrounding_whitespace(&mut root);
    pass_insert_missing_whitespace(&mut root);
    pass_unindent(&mut root);
    pass_indent(&mut root);
    pass_remove_trailing_whitespace(&mut root);
    let out = root.full_text();
    if sep == "\r\n" {
        Ok(out.replace('\n', "\r\n"))
    } else {
        Ok(out)
    }
}


fn line_separator(text: &str) -> &'static str {
    match text.match_indices('\n').next() {
        Some((i, _)) if i > 0 && text.as_bytes()[i - 1] == b'\r' => "\r\n",
        _ => "\n",
    }
}

// ---------------------------------------------------------------------------
// Zipper helpers (path-based)
// ---------------------------------------------------------------------------

fn node_at<'a>(root: &'a Node, path: &[usize]) -> &'a Node {
    let mut cur = root;
    for &i in path {
        cur = &cur.children[i];
    }
    cur
}

fn node_at_mut<'a>(root: &'a mut Node, path: &[usize]) -> &'a mut Node {
    let mut cur = root;
    for &i in path {
        cur = &mut cur.children[i];
    }
    cur
}

fn kids<'a>(root: &'a Node, path: &[usize]) -> &'a [Node] {
    &node_at(root, path).children
}

fn parent_path(path: &[usize]) -> Option<&[usize]> {
    if path.is_empty() {
        None
    } else {
        Some(&path[..path.len() - 1])
    }
}

fn dfs_next(root: &Node, path: &[usize]) -> Option<Vec<usize>> {
    let mut p = path.to_vec();
    if !node_at(root, &p).children.is_empty() {
        p.push(0);
        return Some(p);
    }
    loop {
        if p.is_empty() {
            return None;
        }
        let last = *p.last().unwrap();
        let idx = p.len() - 1;
        let count = node_at(root, &p[..idx]).children.len();
        if last + 1 < count {
            p[idx] = last + 1;
            return Some(p);
        }
        p.pop();
    }
}

fn dfs_prev(root: &Node, path: &[usize]) -> Option<Vec<usize>> {
    if path.is_empty() {
        return None;
    }
    let mut p = path.to_vec();
    if let Some(last) = p.last_mut() {
        if *last > 0 {
            *last -= 1;
            while !node_at(root, &p).children.is_empty() {
                p.push(node_at(root, &p).children.len() - 1);
            }
            return Some(p);
        }
    }
    p.pop();
    Some(p)
}

fn remove_position(root: &Node, path: &[usize]) -> Vec<usize> {
    let last = *path.last().unwrap_or(&0);
    if last > 0 {
        let mut p = path.to_vec();
        *p.last_mut().unwrap() = last - 1;
        while !node_at(root, &p).children.is_empty() {
            p.push(node_at(root, &p).children.len() - 1);
        }
        p
    } else {
        parent_path(path).map(|p| p.to_vec()).unwrap_or_default()
    }
}

fn remove_child(root: &mut Node, path: &[usize]) {
    let pp = &path[..path.len() - 1];
    let i = *path.last().unwrap();
    node_at_mut(root, pp).children.remove(i);
    // The removed node's own width/after-line caches are gone with it;
    // invalidate the parent and its ancestors.
    invalidate_ancestors(root, pp);
}

fn insert_before(root: &mut Node, target: &[usize], n: Node) {
    if target.is_empty() {
        root.children.insert(0, n);
        invalidate_ancestors(root, &[]);
        return;
    }
    let pp = &target[..target.len() - 1];
    let i = *target.last().unwrap();
    node_at_mut(root, pp).children.insert(i, n);
    invalidate_ancestors(root, target);
}

fn elem_left(kids: &[Node], i: usize) -> Option<usize> {
    (0..i).rev().find(|&j| !is_ws_or_comment(kids[j].tag))
}
fn elem_right(kids: &[Node], i: usize) -> Option<usize> {
    (i + 1..kids.len()).find(|&j| !is_ws_or_comment(kids[j].tag))
}
fn leftmost_elem(kids: &[Node]) -> Option<usize> {
    (0..kids.len()).find(|&j| !is_ws_or_comment(kids[j].tag))
}

fn index_of(kids: &[Node], i: usize) -> usize {
    (0..i).filter(|&j| !is_ws_or_comment(kids[j].tag) && !is_uneval(kids[j].tag)).count()
}

// ---------------------------------------------------------------------------
// Margin (rewrite-clj `z/margin` — prior-line-string width, utf16 units)
// ---------------------------------------------------------------------------

fn margin(root: &Node, path: &[usize]) -> usize {
    // prior-line-string (rewrite-clj/cljfmt): walk raw left siblings until a
    // segment containing a newline; when a level has no left siblings, the
    // parent's start element is consumed instead. We collect (width,
    // after-line) segments in final text order and fold left-to-right:
    // text before the last newline is dropped, text after it is summed.
    let mut segs: Vec<(usize, Option<usize>)> = vec![];
    let mut cur = path;
    while let Some(pp) = parent_path(cur) {
        let i = cur.last().copied().unwrap_or(0);
        let k = kids(root, pp);
        if i > 0 {
            for j in (0..i).rev() {
                segs.push((k[j].width(), k[j].after_line()));
            }
        }
        let prefix = node_at(root, pp).text.as_str();
        if !prefix.is_empty() {
            segs.push((utf16_width(prefix), None));
        }
        cur = pp;
    }
    // Fold L→R: a segment containing a newline resets the running width to
    // its after-last-newline width; a newline-free segment adds its width
    // (either before any newline is found, or after the last one seen).
    let mut width = 0usize;
    for (w, al) in segs.into_iter().rev() {
        match al {
            Some(x) => width = x,
            None => width += w,
        }
    }
    width
}

// ---------------------------------------------------------------------------
// Indents table (cljfmt 0.16.6 defaults: clojure.clj + compojure.clj +
// fuzzy.clj)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuleKey {
    Sym(&'static str),
    /// Qualified key (clojure.clj `clojure.core/*` entries): matched against
    /// the fully-qualified symbol (ns-qualified via the file's `:ns-name`
    /// context when unqualified).
    ReDef,   // ^def(?!ault)(?!ate)(?!er)
    ReWith, // ^with-
}

#[derive(Debug, Clone, Copy)]
enum RuleOpt {
    Block(usize),
    Inner(usize, Option<usize>),
}

struct Rule {
    key: RuleKey,
    opts: &'static [RuleOpt],
}


const DEFAULT_RULES: &[Rule] = &[
    // clojure.clj (cljfmt 0.16.6, verbatim)
    Rule { key: RuleKey::Sym("alt!"), opts: &[RuleOpt::Block(0)] },
    Rule { key: RuleKey::Sym("alt!!"), opts: &[RuleOpt::Block(0)] },
    Rule { key: RuleKey::Sym("are"), opts: &[RuleOpt::Block(2)] },
    Rule { key: RuleKey::Sym("as->"), opts: &[RuleOpt::Block(2)] },
    Rule { key: RuleKey::Sym("binding"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("bound-fn"), opts: &[RuleOpt::Inner(0, None)] },
    Rule { key: RuleKey::Sym("case"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("catch"), opts: &[RuleOpt::Block(2)] },
    Rule { key: RuleKey::Sym("comment"), opts: &[RuleOpt::Block(0)] },
    Rule { key: RuleKey::Sym("cond"), opts: &[RuleOpt::Block(0)] },
    Rule { key: RuleKey::Sym("condp"), opts: &[RuleOpt::Block(2)] },
    Rule { key: RuleKey::Sym("cond->"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("cond->>"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("def"), opts: &[RuleOpt::Inner(0, None)] },
    Rule { key: RuleKey::Sym("defmacro"), opts: &[RuleOpt::Inner(0, None)] },
    Rule { key: RuleKey::Sym("defmethod"), opts: &[RuleOpt::Inner(0, None)] },
    Rule { key: RuleKey::Sym("defmulti"), opts: &[RuleOpt::Inner(0, None)] },
    Rule { key: RuleKey::Sym("defn"), opts: &[RuleOpt::Inner(0, None)] },
    Rule { key: RuleKey::Sym("defn-"), opts: &[RuleOpt::Inner(0, None)] },
    Rule { key: RuleKey::Sym("defonce"), opts: &[RuleOpt::Inner(0, None)] },
    Rule { key: RuleKey::Sym("defprotocol"), opts: &[RuleOpt::Block(1), RuleOpt::Inner(1, None)] },
    Rule { key: RuleKey::Sym("defrecord"), opts: &[RuleOpt::Block(2), RuleOpt::Inner(1, None)] },
    Rule { key: RuleKey::Sym("defstruct"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("deftest"), opts: &[RuleOpt::Inner(0, None)] },
    Rule { key: RuleKey::Sym("deftype"), opts: &[RuleOpt::Block(2), RuleOpt::Inner(1, None)] },
    Rule { key: RuleKey::Sym("delay"), opts: &[RuleOpt::Block(0)] },
    Rule { key: RuleKey::Sym("do"), opts: &[RuleOpt::Block(0)] },
    Rule { key: RuleKey::Sym("doseq"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("dotimes"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("doto"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("extend"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("extend-protocol"), opts: &[RuleOpt::Block(1), RuleOpt::Inner(1, None)] },
    Rule { key: RuleKey::Sym("extend-type"), opts: &[RuleOpt::Block(1), RuleOpt::Inner(1, None)] },
    Rule { key: RuleKey::Sym("fdef"), opts: &[RuleOpt::Inner(0, None)] },
    Rule { key: RuleKey::Sym("finally"), opts: &[RuleOpt::Block(0)] },
    Rule { key: RuleKey::Sym("fn"), opts: &[RuleOpt::Inner(0, None)] },
    Rule { key: RuleKey::Sym("for"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("future"), opts: &[RuleOpt::Block(0)] },
    Rule { key: RuleKey::Sym("go"), opts: &[RuleOpt::Block(0)] },
    Rule { key: RuleKey::Sym("go-loop"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("if"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("if-let"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("if-not"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("if-some"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("let"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("let*"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("letfn"), opts: &[RuleOpt::Block(1), RuleOpt::Inner(2, Some(0))] },
    Rule { key: RuleKey::Sym("locking"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("loop"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("match"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("ns"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("proxy"), opts: &[RuleOpt::Block(2), RuleOpt::Inner(1, None)] },
    Rule { key: RuleKey::Sym("reify"), opts: &[RuleOpt::Inner(0, None), RuleOpt::Inner(1, None)] },
    Rule { key: RuleKey::Sym("struct-map"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("testing"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("thread"), opts: &[RuleOpt::Block(0)] },
    Rule { key: RuleKey::Sym("try"), opts: &[RuleOpt::Block(0)] },
    Rule { key: RuleKey::Sym("use-fixtures"), opts: &[RuleOpt::Inner(0, None)] },
    Rule { key: RuleKey::Sym("when"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("when-first"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("when-let"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("when-not"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("when-some"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("while"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("with-local-vars"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("with-open"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("with-out-str"), opts: &[RuleOpt::Block(0)] },
    Rule { key: RuleKey::Sym("with-precision"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("with-redefs"), opts: &[RuleOpt::Block(1)] },
    // compojure.clj (cljfmt 0.16.6, verbatim)
    Rule { key: RuleKey::Sym("ANY"), opts: &[RuleOpt::Inner(0, None)] },
    Rule { key: RuleKey::Sym("DELETE"), opts: &[RuleOpt::Inner(0, None)] },
    Rule { key: RuleKey::Sym("GET"), opts: &[RuleOpt::Inner(0, None)] },
    Rule { key: RuleKey::Sym("HEAD"), opts: &[RuleOpt::Inner(0, None)] },
    Rule { key: RuleKey::Sym("OPTIONS"), opts: &[RuleOpt::Inner(0, None)] },
    Rule { key: RuleKey::Sym("PATCH"), opts: &[RuleOpt::Inner(0, None)] },
    Rule { key: RuleKey::Sym("POST"), opts: &[RuleOpt::Inner(0, None)] },
    Rule { key: RuleKey::Sym("PUT"), opts: &[RuleOpt::Inner(0, None)] },
    Rule { key: RuleKey::Sym("context"), opts: &[RuleOpt::Inner(0, None)] },
    Rule { key: RuleKey::Sym("defroutes"), opts: &[RuleOpt::Inner(0, None)] },
    Rule { key: RuleKey::Sym("let-routes"), opts: &[RuleOpt::Block(1)] },
    Rule { key: RuleKey::Sym("rfn"), opts: &[RuleOpt::Inner(0, None)] },
    // fuzzy.clj (cljfmt 0.16.6, verbatim)
    Rule { key: RuleKey::ReDef, opts: &[RuleOpt::Inner(0, None)] },
    Rule { key: RuleKey::ReWith, opts: &[RuleOpt::Inner(0, None)] },
];

fn rule_sort_key(r: &Rule) -> (usize, u8, &'static str) {
    let max_depth = r
        .opts
        .iter()
        .map(|o| match o {
            RuleOpt::Block(_) => 0,
            RuleOpt::Inner(d, _) => *d,
        })
        .max()
        .unwrap_or(0);
    let (kind, name) = match r.key {
        RuleKey::Sym(s) => (1u8, s),
        RuleKey::ReDef => (2u8, "^def(?!ault)(?!late)(?!er)"),
        RuleKey::ReWith => (2u8, "^with-"),
    };
    (max_depth, kind, name)
}

// ---------------------------------------------------------------------------
// Indent computation
// ---------------------------------------------------------------------------

fn reader_conditional(root: &Node, path: &[usize]) -> bool {
    let n = node_at(root, path);
    if n.tag != Tag::ReaderMacro {
        return false;
    }
    n.children
        .first()
        .is_some_and(|c| c.tag == Tag::Token && (c.text == "?" || c.text == "?@"))
}

fn is_symbol_token(n: &Node) -> Option<&str> {
    if n.tag != Tag::Token {
        return None;
    }
    let t = n.text.as_str();
    if t.starts_with('\\') {
        return None; // char literal
    }
    match classify_token(t) {
        Ok(TokVal::Sym) => Some(t),
        _ => None,
    }
}

fn key_matches(key: RuleKey, sym: &str) -> bool {
    match key {
        RuleKey::Sym(k) => sym_name(sym) == k,
        RuleKey::ReDef => {
            // ^def(?!ault)(?!late)(?!er)
            let name = sym_name(sym);
            if !name.starts_with("def") {
                return false;
            }
            let rest = &name[3..];
            !rest.starts_with("ault") && !rest.starts_with("late") && !rest.starts_with("er")
        }
        RuleKey::ReWith => sym_name(sym).starts_with("with-"),
    }
}

fn sym_name(s: &str) -> &str {
    s.rfind('/').map(|p| &s[p + 1..]).unwrap_or(s)
}

fn form_symbol(root: &Node, path: &[usize]) -> Option<String> {
    let pp = parent_path(path)?;
    let k = kids(root, pp);
    let _i = *path.last()?;
    let z0 = leftmost_elem(k)?;
    let mut z: Vec<usize> = pp.to_vec();
    z.push(z0);
    // skip-meta
    loop {
        let n = node_at(root, &z);
        if is_meta(n.tag) {
            let ck = &n.children;
            let d = leftmost_elem(ck)?;
            let r = elem_right(ck, d)?;
            z.pop();
            z.push(r);
        } else {
            break;
        }
    }
    let n = node_at(root, &z);
    if let Some(t) = is_symbol_token(n) {
        return Some(t.to_string());
    }
    if n.tag == Tag::ReaderMacro && reader_conditional(root, &z) {
        return first_symbol_in_reader_conditional(root, &z);
    }
    None
}

fn first_symbol_in_reader_conditional(root: &Node, z: &[usize]) -> Option<String> {
    // (-> zloc z/down z/right z/down find-next-keyword) → key; value after
    let ck = kids(root, z);
    let d = leftmost_elem(ck)?;
    let r = elem_right(ck, d)?;
    let rpath = {
        let mut v = z.to_vec();
        v.push(r);
        v
    };
    let rd_kids = kids(root, &rpath);
    let kw_start = leftmost_elem(rd_kids)?;
    let kw_idx = (kw_start..rd_kids.len()).find(|&j| rd_kids[j].tag == Tag::Token && rd_kids[j].text.starts_with(':'))?;
    let val_idx = elem_right(rd_kids, kw_idx)?;
    let val_path = {
        let mut v = z.to_vec();
        v.push(r);
        v.push(val_idx);
        v
    };
    is_symbol_token(node_at(root, &val_path)).map(|t| t.to_string())
}

fn form_matches(root: &Node, path: &[usize], key: RuleKey) -> bool {
    form_symbol(root, path).is_some_and(|sym| key_matches(key, &sym))
}

fn list_indent(root: &Node, path: &[usize]) -> usize {
    let pp = parent_path(path).unwrap_or(&[]);
    let k = kids(root, pp);
    let i = *path.last().unwrap_or(&0);
    if index_of(k, i) > 1 {
        let second = elem_right(k, 0).unwrap_or(0);
        let spath = {
            let mut v = pp.to_vec();
            v.push(second);
            v
        };
        margin(root, &spath)
    } else {
        let spath = {
            let mut v = pp.to_vec();
            v.push(0);
            v
        };
        margin(root, &spath)
    }
}

fn first_form_in_line(root: &Node, path: &[usize]) -> bool {
    let pp = parent_path(path).unwrap_or(&[]);
    let k = kids(root, pp);
    let i = *path.last().unwrap_or(&0);
    let mut j = i;
    while j > 0 {
        j -= 1;
        match k[j].tag {
            Tag::Ws => {}
            Tag::Newline | Tag::Comment => return true,
            _ => return false,
        }
    }
    true
}

fn inner_indent(root: &Node, path: &[usize], key: RuleKey, depth: usize, idx: Option<usize>) -> Option<usize> {
    // depth 0: top is the zloc itself (the linebreak) — its form is the
    // parent's leftmost element (form-symbol walks the parent).
    if path.len() < depth {
        return None;
    }
    let top_path = &path[..path.len() - depth];
    if !form_matches(root, top_path, key) {
        return None;
    }
    let pp = parent_path(path)?;
    let k = kids(root, pp);
    let i = *path.last()?;
    elem_left(k, i)?;
    if let Some(idx) = idx {
        // index-matches-top-argument?: index-of the TOP node's siblings
        let tp = parent_path(top_path).unwrap_or(&[]);
        let tk = kids(root, tp);
        let ti = top_path.last().copied().unwrap_or(0);
        if idx + 1 != index_of(tk, ti) {
            return None;
        }
    }
    Some(margin(root, pp) + indent_width(node_at(root, pp).tag))
}

fn indent_width(tag: Tag) -> usize {
    match tag {
        Tag::Fn => 3,
        _ => 2,
    }
}

fn block_indent(root: &Node, path: &[usize], key: RuleKey, idx: usize) -> Option<usize> {
    if !form_matches(root, path, key) {
        return None;
    }
    let pp = parent_path(path).unwrap_or(&[]);
    let k = kids(root, pp);
    let i = *path.last().unwrap_or(&0);
    // nth-form(zloc, (inc idx)): from the leftmost element, idx+1 z/right moves;
    // nil when the move runs off the end (→ inner indent branch, like cljfmt).
    let mut j = leftmost_elem(k)?;
    let mut after: Option<Vec<usize>> = Some({
        let mut v = pp.to_vec();
        v.push(j);
        v
    });
    for _ in 0..=idx {
        match elem_right(k, j) {
            Some(rj) => {
                j = rj;
                if let Some(a) = after.as_mut() {
                    a.truncate(a.len() - 1);
                    a.push(rj);
                }
            }
            None => {
                after = None;
                break;
            }
        }
    }
    let after_ok = after.is_none() || first_form_in_line(root, after.as_ref().unwrap());
    if after_ok && index_of(k, i) > idx {
        inner_indent(root, path, key, 0, None)
    } else {
        Some(list_indent(root, path))
    }
}

fn indenter(root: &Node, path: &[usize]) -> Option<usize> {
    let mut rules: Vec<&Rule> = DEFAULT_RULES.iter().collect();
    // cljfmt indent-order: max-depth desc, then key kind (qualified 0, simple
    // 1, pattern 2) asc, then (str key) asc.
    rules.sort_by(|a, b| {
        let ka = rule_sort_key(a);
        let kb = rule_sort_key(b);
        kb.0.cmp(&ka.0)
            .then(ka.1.cmp(&kb.1))
            .then_with(|| ka.2.cmp(kb.2))
    });
    for r in rules {
        for opt in r.opts {
            match opt {
                RuleOpt::Block(idx) => {
                    if let Some(w) = block_indent(root, path, r.key, *idx) {
                        return Some(w);
                    }
                }
                RuleOpt::Inner(depth, idx) => {
                    if let Some(w) = inner_indent(root, path, r.key, *depth, *idx) {
                        return Some(w);
                    }
                }
            }
        }
    }
    None
}

fn indent_amount(root: &Node, path: &[usize]) -> usize {
    let pp = parent_path(path).unwrap_or(&[]);
    if let Some(gp) = parent_path(pp) {
        if reader_conditional(root, gp) {
            let mut spath = path.to_vec();
            *spath.last_mut().unwrap() = 0;
            return margin(root, &spath);
        }
    }
    match node_at(root, pp).tag {
        Tag::List | Tag::Fn => indenter(root, path).unwrap_or_else(|| list_indent(root, path)),
        _ => {
            let mut spath = path.to_vec();
            *spath.last_mut().unwrap() = 0;
            margin(root, &spath)
        }
    }
}

// ---------------------------------------------------------------------------
// Passes
// ---------------------------------------------------------------------------

fn edit_all(
    root: &mut Node,
    mut p: impl FnMut(&Node, &[usize]) -> bool,
    mut f: impl FnMut(&mut Node, &[usize]) -> Vec<usize>,
) {
    // of-node: first non-ws/non-comment child, or the forms root itself when
    // the file has no elements.
    let mut cur: Option<Vec<usize>> = match leftmost_elem(&root.children) {
        Some(i) => Some(vec![i]),
        None => Some(vec![]),
    };
    while let Some(c) = cur {
        // edit-all: zloc = (if (p? zloc) (f zloc) zloc)
        let pos = if p(root, &c) { f(root, &c) } else { c };
        // then find the next p?-positive node strictly after `pos`
        let mut q = dfs_next(root, &pos);
        while let Some(qc) = q.clone() {
            if p(root, &qc) {
                break;
            }
            q = dfs_next(root, &qc);
        }
        cur = q;
    }
}

fn skip_wc_next(root: &Node, path: &[usize]) -> Option<Vec<usize>> {
    let mut z = path.to_vec();
    loop {
        let n = node_at(root, &z);
        if is_clojure_ws(n.tag) {
            z = dfs_next(root, &z)?;
        } else {
            return Some(z);
        }
    }
}

/// Count newlines in the run at `path` (cljfmt `count-newlines`):
/// walk right* (raw) skipping ws/comma via depth-first next while the
/// position is a newline; when it stops (nil or non-newline), the ORIGINAL
/// zloc's left (skipping clojure-ws raw) is checked for an adjacent comment.
fn count_newlines(root: &Node, path: &[usize]) -> usize {
    let mut count = 0usize;
    let mut z = path.to_vec();
    loop {
        let n = node_at(root, &z);
        if n.tag != Tag::Newline {
            break;
        }
        count += n.text.len();
        // right* (raw right sibling), then skip ws/comma via next*
        let r = match parent_path(&z) {
            Some(pp) => {
                let i = *z.last().unwrap();
                let k = kids(root, pp);
                (i + 1 < k.len()).then(|| {
                    let mut v = pp.to_vec();
                    v.push(i + 1);
                    v
                })
            }
            None => None,
        };
        match r {
            Some(r) => match skip_wc_next(root, &r) {
                Some(r2) => z = r2,
                None => break, // ran off the end → nil → else branch
            },
            None => break, // right* is nil → nil → else branch
        }
    }
    // else branch: comment? (skip-clojure-whitespace zloc z/left*) — walk raw
    // left from the original zloc while clojure-ws
    if let Some(pp) = parent_path(path) {
        let i = *path.last().unwrap_or(&0);
        let k = kids(root, pp);
        let mut j = i;
        while j > 0 && is_clojure_ws(k[j].tag) {
            j -= 1;
        }
        // j now at the first non-clojure-ws left sibling (or 0); the walk also
        // passes through k[0] when it is clojure-ws (→ nil → no comment)
        if !is_clojure_ws(k[j].tag) && is_comment(k[j].tag) {
            count += 1;
        }
    }
    count
}

fn final_transform(root: &Node, path: &[usize]) -> bool {
    let mut z = match dfs_next(root, path) {
        Some(z) => z,
        None => return true,
    };
    loop {
        let node = node_at(root, &z);
        if !is_clojure_ws(node.tag) {
            return false;
        }
        match dfs_next(root, &z) {
            Some(z2) => z = z2,
            None => return true,
        }
    }
}

fn pass_consecutive_blank_lines(root: &mut Node) {
    edit_all(root, |root, path| {
        let n = node_at(root, path);
        (n.tag == Tag::Newline || is_comment(n.tag)) && count_newlines(root, path) > 2
            && !final_transform(root, path)
    }, |root, path| {
        // replace-consecutive-blank-lines:
        //   e      = skip-clojure-whitespace(zloc)      (next*, from zloc)
        //   pos    = z/prev* e                          (DFS-prev; parent when first)
        //   before = remove-clojure-whitespace(pos)     (drop the cw run)
        //   insert n newlines left of z/next* before; n = 1 iff before is a
        //          comment. Position ends at the insertion anchor.
        let e = skip_wc_next(root, path).expect("p? guarantees a following element");
        let mut pos = dfs_prev(root, &e).unwrap_or_default();
        while is_clojure_ws(node_at(root, &pos).tag) {
            // position after remove*: DFS-prev of the removed node (deepest
            // rightmost left sibling, or the parent)
            let new_pos = remove_position(root, &pos);
            remove_child(root, &pos);
            pos = new_pos;
        }
        let newlines = if is_comment(node_at(root, &pos).tag) { 1 } else { 2 };
        let target = dfs_next(root, &pos).unwrap_or_else(|| pos.clone());
        insert_before(root, &target, Node::leaf(Tag::Newline, "\n".repeat(newlines)));
        target
    });
}

fn pass_remove_surrounding_whitespace(root: &mut Node) {
    edit_all(root, |root, path| {
        let n = node_at(root, path);
        if !is_clojure_ws(n.tag) {
            return false;
        }
        // not top-level (top? = parent is the forms root)
        if path.len() == 1 {
            return false;
        }
        let pp = parent_path(path).unwrap();
        let k = kids(root, pp);
        let i = *path.last().unwrap();
        if i == 0 {
            // left* is nil: remove unless right* is a comment or an
            // unquote-deref (a deref whose raw parent is an unquote: `~ @x`)
            if (i + 1) < k.len() {
                let rt = k[i + 1].tag;
                if rt == Tag::Comment {
                    return false;
                }
                if rt == Tag::Deref && node_at(root, pp).tag == Tag::Unquote {
                    return false;
                }
            }
            return true;
        }
        // right side: z/skip z/right* clojure-whitespace? reaches the end
        // (a comment or element stops the walk)
        let mut j = i + 1;
        while j < k.len() && is_clojure_ws(k[j].tag) {
            j += 1;
        }
        j >= k.len()
    }, |root, path| {
        remove_child(root, path);
        remove_position(root, path)
    });
}

fn pass_insert_missing_whitespace(root: &mut Node) {
    edit_all(root, |root, path| {
        let n = node_at(root, path);
        if !is_element(n.tag) {
            return false;
        }
        let pp = parent_path(path).unwrap_or(&[]);
        let pt = node_at(root, pp).tag;
        if pt == Tag::ReaderMacro || pt == Tag::NamespacedMap {
            return false;
        }
        let k = kids(root, pp);
        let i = *path.last().unwrap_or(&0);
        (i + 1 < k.len()) && is_element(k[i + 1].tag)
    }, |root, path| {
        let _pp = parent_path(path).unwrap_or(&[]);
        let _i = *path.last().unwrap_or(&0);
        let mut target = path.to_vec();
        target.push(0); // placeholder; we insert after i in parent
        let _ = target;
        insert_after(root, path, Node::leaf(Tag::Ws, " ".into()));
        path.to_vec()
    });
}

fn insert_after(root: &mut Node, path: &[usize], n: Node) {
    let pp = &path[..path.len() - 1];
    let i = *path.last().unwrap();
    node_at_mut(root, pp).children.insert(i + 1, n);
    invalidate_ancestors(root, path);
}

fn comment_next(root: &Node, path: &[usize]) -> bool {
    // (-> zloc z/next* skip-whitespace comment?) — skip-whitespace moves by
    // z/next* while the node is a :whitespace (space) node only.
    let mut z = match dfs_next(root, path) {
        Some(z) => z,
        None => return false,
    };
    loop {
        let n = node_at(root, &z);
        if n.tag != Tag::Ws {
            return n.tag == Tag::Comment;
        }
        match dfs_next(root, &z) {
            Some(z2) => z = z2,
            None => return false,
        }
    }
}

fn pass_unindent(root: &mut Node) {
    edit_all(root, |root, path| {
        let n = node_at(root, path);
        if n.tag != Tag::Ws {
            return false;
        }
        let pp = parent_path(path).unwrap_or(&[]);
        let k = kids(root, pp);
        let i = *path.last().unwrap_or(&0);
        if i == 0 {
            return false;
        }
        matches!(k[i - 1].tag, Tag::Newline | Tag::Comment) && !comment_next(root, path)
    }, |root, path| {
        remove_child(root, path);
        remove_position(root, path)
    });
}

fn pass_indent(root: &mut Node) {
    edit_all(root, |root, path| {
        let n = node_at(root, path);
        (n.tag == Tag::Newline || is_comment(n.tag)) && !comment_next(root, path)
    }, |root, path| {
        let width = indent_amount(root, path);
        if width > 0 {
            insert_after(root, path, Node::leaf(Tag::Ws, " ".repeat(width)));
        }
        path.to_vec()
    });
}

fn pass_remove_trailing_whitespace(root: &mut Node) {
    edit_all(root, |root, path| {
        let n = node_at(root, path);
        if n.tag != Tag::Ws {
            return false;
        }
        let pp = parent_path(path).unwrap_or(&[]);
        let k = kids(root, pp);
        let i = *path.last().unwrap_or(&0);
        let right = (i + 1 < k.len()).then(|| k[i + 1].tag);
        right == Some(Tag::Newline) || (right.is_none() && path.len() == 1)
    }, |root, path| {
        remove_child(root, path);
        remove_position(root, path)
    });
}

