(defroutes
  (context "/api" []
    (GET "/items" [] (items))
    (POST "/items" [body] (add-item body)))
  (let-routes [user]
    (GET "/me" [] (me user))))
(def w (with-meta {:a 1} {:doc "x"}))
(defn- helper [x] (inc x))
(ns my.app
  (:require [clojure.string :as str]
            [other.lib :refer [foo bar] :rename {baz -> quux}])
  (:import (java.util Date)))
(def ^:dynamic *v* 0)
(def ratio 1/3)
(def hex 0xFF)
(def exp 1.5e10)
(def big 12345678901234567890)
