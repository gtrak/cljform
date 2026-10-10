(defn multi
  ([a] (one a))
  ([a b] (two a b))
  ([a b & rest] (many a b rest)))
(let [x (let [y (let [z 1] z)] y)]
  (inc x))
(if (and a (or b c))
  (do
    (x)
    (y))
  (z))
