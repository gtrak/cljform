(defm macro-style
  [x]
  `(do ~@x))
(dosync (commute ref (fn [v] (conj v 1))))
(delay (expensive 1))
(doseq [x [1 2] y [3 4]]
  (printf "%s %s\n" x y))
(locking obj (with-rented-transaction db (do-thing db)))
(bound-fn (fn [a] a) b 1 c 2)
(defn inc* [n] (if (pos? n) (recur (dec n)) 0))
(case x 1 :one 2 :two :else)
(merge {:a 1}
       {:b 2}
       {:c 3})
