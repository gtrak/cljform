(letfn [(rec [n] (if (zero? n) 0 (inc (rec (dec n)))))]
  (rec 5))
