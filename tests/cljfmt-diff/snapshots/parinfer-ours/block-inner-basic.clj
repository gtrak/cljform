(ns foo)
(defn add
  [x y]
  (+ x y))
(let [a 1
      b 2]
  (do a
      b))
(def x 1)
(defn f [a]
  (g a
     b))
(let [x 1]
 (x))
(if a b
    c)
(when cond
  body)
