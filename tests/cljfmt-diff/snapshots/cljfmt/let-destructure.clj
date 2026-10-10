(ns m)
(defn f [a b]
  (if (zero? a)
    b
    (recur (dec a) (inc b))))
(let [{x :a :as m} (get-in data [:k])
      y (str x)]
  (println y m))
