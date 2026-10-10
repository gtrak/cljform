(def s "keep   (this  as-is\n  indented   ")
(def re #"a ( b")
(def ch \))
(def cs \[ \{ \\)
(def str2 "with \t tab and \n nl and \" quote")
(defn g [x]
  (str "a" x "b"))
