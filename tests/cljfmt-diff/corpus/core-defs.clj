(defn- inner-fn [x]
  (x))
(defmulti m :x)
(defmethod m :a [x] x)
(letfn [(rec [n] (if (zero? n) 0 (inc (rec (dec n)))))]
  (rec 5))
(defrecord P [a b])
(deftype T [v])
(defprotocol P2 (m [x]))
(reify Object
  (toString [_] "hi"))
(proxy Object [] Object (toString [] "p"))
(extend-type String)
(defstruct S [a])
(defmacro mm [& xs] xs)
