#!/usr/bin/env bb

;; Pure file-length checker.
;;
;; Usable two ways:
;;   - standalone:  bb bb/length.clj [--warn N] [--fail N] <file-or-dir>...
;;   - as a library: (require '[length]) and call `check-file` / `check-paths`,
;;     which return result maps instead of printing.

(ns length
  (:require [babashka.fs :as fs]))

(defn check-file
  "Check a single file against warn/fail line thresholds.
  Returns a result map: {:status :ok|:warn|:fail|:skip :path s :lines n :limit n}.
  Unreadable files (binary, permissions, ...) come back as :skip."
  [path warn fail]
  (try
    (let [lines (count (fs/read-all-lines path))]
      (cond
        (>= lines fail) {:status :fail :path (str path) :lines lines :limit fail}
        (>= lines warn) {:status :warn :path (str path) :lines lines :limit warn}
        :else           {:status :ok   :path (str path) :lines lines}))
    (catch Exception _
      {:status :skip :path (str path)})))

(defn check-paths
  "Check many paths (files only) against the same thresholds."
  [paths warn fail]
  (map #(check-file % warn fail) paths))

(defn report
  "Print a single result line. :ok and :skip are silent.
  Both messages point at splitting the file, not shaving lines to fit budget."
  [{:keys [status path lines limit]}]
  (case status
    :fail (println (str "SPLIT: " path " (" lines " lines, over limit " limit ") "
                       "— too big, break it into smaller files"))
    :warn (println (str "WARN:  " path " (" lines " lines, limit " limit ") "
                       "— getting large, consider splitting it (don't shave lines to fit)"))
    nil))

(defn- expand [arg]
  (if (fs/directory? arg)
    (filter fs/regular-file? (fs/glob arg "**"))
    [(fs/path arg)]))

(defn- parse-args [args]
  (loop [args args
         opts {:warn 800 :fail 1200 :paths []}]
    (if-let [a (first args)]
      (case a
        "--warn" (recur (drop 2 args) (assoc opts :warn (parse-long (second args))))
        "--fail" (recur (drop 2 args) (assoc opts :fail (parse-long (second args))))
        (recur (rest args) (update opts :paths conj a)))
      opts)))

(defn -main [args]
  (let [{:keys [warn fail paths]} (parse-args args)]
    (when (empty? paths)
      (println "Usage: bb bb/length.clj [--warn N] [--fail N] <file-or-dir>...")
      (System/exit 1))
    (let [results (check-paths (mapcat expand paths) warn fail)
          fails   (filter #(= :fail (:status %)) results)]
      (run! report results)
      (when (seq fails)
        (System/exit 1)))))

;; Run the CLI only when invoked directly, not when required as a library.
(when (= *file* (System/getProperty "babashka.file"))
  (-main *command-line-args*))
