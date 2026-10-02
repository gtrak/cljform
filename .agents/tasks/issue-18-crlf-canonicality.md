# Issue 18 — C-1: edit adopts content's line endings, silently breaking format-canonicality
T12 finding (independently confirmed). An edit whose content uses LF into a
CRLF file writes LF lines (od-verified mixed endings); the file was
format-canonical before and is not after — and the edit output carries no
note. Asymmetric: CRLF content into an LF file... CRLF content is preserved.
"cljform wrote it" must imply "it is formatted" — this breaks the invariant
for the CRLF corpus.
Fix direction (design decision to confirm): prepare/base-shift should adopt
the FILE's dominant line ending for spliced content (normalize content EOLs
to the file's style before parinfer/reindent). Alternative: hard note in the
edit output when endings are mixed (weaker; still non-canonical).
Repro: /tmp/press/torture/c2.clj flow (printf '(defn one\r\n...' > c2.clj;
format ok; edit --handle H --content $'...LF...'; format -> diff).
