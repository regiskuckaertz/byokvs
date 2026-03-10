(ns w1
  (:require [reitit.ring :as r]
            [org.httpkit.client :as http]
            [org.httpkit.server :as hk-server]
            [clojure.java.io :as io]
            [clojure.string :as str]
            [clojure.pprint]
            [clojure.core.match :refer [match]]
            [ring.util.request]
            [bigml.histogram.core :as histogram]))

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

(defn latency [h fn]
  (let [start (System/nanoTime)
        result (fn)
        end (System/nanoTime)]
    (histogram/insert! h (- end start))
    result))

(defn run [_]
  (let [h (histogram/create)
        cache (ref {})
        kill (hk-server/run-server (server cache) {:port 8080})]
    (println "Loading put.txt...")
    (with-open [spec (io/reader "put.txt")]
      (println "Running requests...")
      (doseq [line (line-seq spec)]
        (match [(str/split line #"\s+")]
          [["PUT" key value]]
          (do (println (str "Putting key " key))
              (let [{:keys [status]} (latency h (fn [] @(http/put (str "http://localhost:8080/" key) {:body value})))]
                (if (== status 204)
                  nil
                  (str "Status: " status))))
          [["GET" key "NOT_FOUND"]]
          (do (println (str "Getting inexistent key " key))
              (let [{:keys [status]} (latency h (fn [] @(http/get (str "http://localhost:8080/" key))))]
                (if (= status 404)
                  nil
                  (throw (Exception. (str "Key found " key))))))
          [["GET" key value]]
          (do (println (str "Getting key " key))
              (let [{:keys [status body]} (latency h (fn [] @(http/get (str "http://localhost:8080/" key) {:as :text})))]
                (if (= status 200)
                  (if (= value body)
                    nil
                    (throw (Exception. (str "Value mismatch for key " key ", expected: " value ", actual: " body))))
                  (throw (Exception. (str "Key not found " key))))))
          :else nil)))

    (println "Ok, done. Latencies (ms):")
    (let [pct (histogram/percentiles h 99.0 95.0 50.0)]
      (println (str "  - p50: " (/ (get pct 50.0) 1000000.0)))
      (println (str "  - p95: " (/ (get pct 95.0) 1000000.0)))
      (println (str "  - p99: " (/ (get pct 99.0) 1000000.0))))

    (kill)))
