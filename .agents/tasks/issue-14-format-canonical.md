# Issue 14 — edit output must be format-canonical; formatting failures must not be silent

From live agent use (28 clj_edit calls): a project format gate caught a file
cljform wrote — parse-valid but **format-non-canonical** (a top-level closing
paren alone on its own line). "cljform wrote it" must imply "it is formatted",
or every consumer needs an external format gate. Root causes found:

- **R1 (format_paren divergence).** Pull-up around comments fails instead of
  formatting. Both of these get `format-error: candidate's token stream
  differs` while installed parinfer-rust (--mode paren) formats them fine:
  - `(defn h [x]\n  (bar x)\n  ;; done\n)\n` → parinfer: `(defn h [x]\n  (bar x))\n  ;; done\n\n`
  - `(defn h [x]\n  (bar x) ;; c\n)\n` → parinfer: `(defn h [x]\n  (bar x)) ;; c\n\n`
  Root-cause the comment/trail interaction in `src/format.rs`
  (`on_comment_line`, `finish_new_paren_trail`, `append_paren_trail`,
  `clean_paren_trail`) and fix it to match the reference.
- **R2 (silent skip).** The edit pipeline (src/main.rs ~1459) applies
  `format_paren`'s candidate only when parse + token-stream verify, and
  otherwise **silently** keeps the unformatted content — no note. Because of
  R1, an agent never learns its edit was written unformatted. Formatting is
  best-effort by design, but never silent: on verification failure push a note
  like `content not reformatted: candidate failed verification (report as
  cljform bug)`; likewise if `format_paren` itself errors.
- **R3 (insert seam displaces parent closers).** Repro (fresh file):
  ```
  (ns a)

  (defn outer [x]
    (let [a 1]
      (when x
        (inner x))))

  (def tail 1)
  ```
  `edit --handle <inner> --mode insert-after --content '(defn helper [x]\n  (bar x)\n  )'`
  writes `      (defn helper [x]\n        (bar x))\n        )))` — the parent
  closers that shared the anchor's line are left alone on their own line
  (here at col 8; in live use at top level, which the project gate caught).
  The closers displaced by the splice must be pulled onto the last line of
  the inserted content (paren-trail semantics at the seam): `…(bar x)))))`.
  Check the same seam shape for nested replace (parent closers after a
  multi-line replacement inside their line).
- **R4 (vacated whitespace-only lines).** Pull-up leaves the vacated line as
  whitespace-only (`        `). parinfer-rust leaves it too, but cljfmt-land
  removes it, and we create the line ourselves. When our pull-up empties a
  line, delete the line. Document this as a deliberate extension beyond
  parinfer in SPEC §10.5; adjust the differential tests to assert our
  documented behavior on exactly these shapes (keep the rest matching the
  reference).

## The invariant (the real gate)
**For a format-canonical input file, every successful edit produces output
`cljform format` treats as a no-op** (candidate == input). Add a loop test:
for a canonical fixture matrix × representative edits (replace, patch,
insert-after, insert-before, delete, append), after the edit, `format` on the
result must emit candidate == content. R1–R4 repros become golden tests.

## Constraints
- I1–I3 (parsable, untouched forms) still hold; the token stream of the whole
  file is unchanged by any formatting we apply.
- Only the changed region (seam + inserted/reindented content + displaced
  closers) may be reformatted — never untouched forms.
- Keep the parinfer-rust differential tests green except where §10.5 now
  documents the R4 extension.

## Gates
`cargo build` · `cargo clippy --all-targets -- -D warnings` · `cargo test` ·
the R1–R4 repros (show before/after).

## Report
Root cause of R1 (which function mishandled the comment); how R3 is scoped so
untouched forms can't move; the golden invariant test name and the matrix it
covers; any differential tests that now assert documented divergence.
