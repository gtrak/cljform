# Issue 24 — M1: materialize refuses balanced input with a col-0/col<=opener closer line
T13 finding (confirmed, pre-existing, predates the shared-lexer refactor).
  printf '(a\n  b\n)\n' | cljform materialize
  -> error [materialize-error] "unmatched ')'" (line 3) while check accepts
  the same content as balanced (1 form). Violates the documented "balanced
  input passes through unchanged" contract. Root cause: the dedent rule
  "open.col >= indent" pre-closes the paren on the closer line, then the
  real ) has nothing to match.
Fix direction: when the line's FIRST code byte is a closer at col <=
opener's col, treat that line's closer as THE form's closer (consume one),
not a dedent-triggered synthetic close; or validate the candidate against
the tree-sitter parse before erroring and pass through when it parses.
Must stay refusal-correct for genuinely unbalanced input.
