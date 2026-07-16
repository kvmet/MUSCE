#!/usr/bin/env bb

;; Project hygiene gate.
;;
;; Walks the given paths (default: the whole project) and applies per-file-type
;; length rules, delegating the actual counting to `length`.
;;
;;   bb bb/hygiene.clj           ; scan from the current directory
;;   bb bb/hygiene.clj ./        ; same
;;   bb bb/hygiene.clj src docs  ; scan specific subtrees

(ns hygiene
  (:require [babashka.fs :as fs]
            [length]
            [guards]))

;; Directories and files to skip entirely.
;; A name here ignores the file, or everything nested inside the directory.
(def ignored
  #{"target"       ; build artefacts
    ".git"
    "Cargo.lock"}) ; generated, not authored

;; Length rules, tried top to bottom; the first match wins.
;; :ext   - required file extension (no leading dot)
;; :under - optional path component that must be present (e.g. "docs")
;; :warn / :fail - line thresholds
(def rules
  [{:ext "md"   :under "docs" :warn 200 :fail 300}   ; docs: keep them tight
   {:ext "md"               :warn 300 :fail 600}
   {:ext "rs"               :warn 800 :fail 1200}
   {:ext "clj"              :warn 400 :fail 800}
   {:ext "toml"             :warn 150 :fail 300}])

(defn- components [path]
  (set (map str (fs/components path))))

(defn ignored? [path]
  (some ignored (components path)))

(defn rule-for [path]
  (let [ext   (fs/extension path)
        comps (components path)]
    (first (filter (fn [{r-ext :ext under :under}]
                     (and (= ext r-ext)
                          (or (nil? under) (contains? comps under))))
                   rules))))

(defn- expand [arg]
  (if (fs/directory? arg)
    (filter fs/regular-file? (fs/glob arg "**"))
    [(fs/path arg)]))

(let [args    *command-line-args*
      roots   (if (empty? args) ["."] args)
      files   (->> (mapcat expand roots)
                   (remove ignored?))
      results (keep (fn [f]
                      (when-let [{:keys [warn fail]} (rule-for f)]
                        (length/check-file f warn fail)))
                    files)
      fails   (filter #(= :fail (:status %)) results)
      warns   (filter #(= :warn (:status %)) results)
      ;; The raw-mutation guard targets the one crate that holds the raw hecs
      ;; handle, regardless of the roots the length gate was pointed at.
      guard-viols (guards/check-root guards/default-root)]

  (run! length/report results)
  (run! guards/report guard-viols)
  (println (str "\n" (count fails) " over budget, " (count warns) " warning(s), "
                (count guard-viols) " unwaived raw borrow(s)"))

  (when (or (seq fails) (seq guard-viols))
    (System/exit 1)))
