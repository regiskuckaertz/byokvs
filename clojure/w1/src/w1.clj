(ns w1
  (:require [reitit.ring :as r]
            [org.httpkit.server :as hk-server]
            [ring.util.request]))

(defn get-key [cache]
  (fn [req]
    (let [params (:path-params req)
          key (:key params)
          value (get @cache key)]
      (cond (nil? value) {:status 404 :body (str "Key " key " not found")}
            :else {:status 200 :body value}))))

(defn put-key [cache]
  (fn [req]
    (let [params (:path-params req)
          key (:key params)
          value (ring.util.request/body-string req)]
      (dosync (alter cache assoc key value))
      {:status 204})))

(defn server [cache]
  (r/ring-handler
   (r/router
    ["/:key" {:get (get-key cache) :put (put-key cache)}])
   (r/create-default-handler)))

(defn run [_]
  (let [cache (ref {})
        kill (hk-server/run-server (server cache) {:port 8080})]
    (println "Server running on port 8080. Press <enter> to stop.")
    (read-line)
    (kill)))
