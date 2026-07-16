#!/usr/bin/env bb

;; Raw-mutation guard.
;;
;; The engine's persistence and tracked-index correctness rest on one rule: a
;; persisted component is only ever changed through a `World` mutator
;; (`modify`/`insert`/`set_component`/...), which marks the entity dirty for the
;; next delta snapshot and emits `ComponentChanged`. A *raw* mutable component
;; borrow off the private hecs handle sidesteps all of that: the write silently
;; vanishes from the next delta and desyncs any tracked index over it. That is the
;; exact bug class the encapsulation made unwritable outside `musce_core`; inside
;; it, the raw handle still exists, so this check keeps the remaining raw borrows
;; down to a handful of consciously-waived sites.
;;
;; A line whose code (comments stripped) reaches a mutable component borrow must
;; carry an explicit `hygiene:allow-raw-mut` waiver, which forces the author to
;; acknowledge they own the `mark_dirty` / trigger bookkeeping by hand. An
;; unwaived borrow anywhere is a failure.
;;
;;   bb bb/guards.clj                 ; scan the default root (musce_core/src)
;;   bb bb/guards.clj musce_core/src  ; scan a specific subtree
;;
;; Also usable as a library: (require '[guards]) and call `check-root`, which
;; returns violation maps instead of exiting.

(ns guards
  (:require [babashka.fs :as fs]
            [clojure.string :as str]))

;; Only `musce_core` wraps the raw `hecs::World`; every other crate sees the World
;; facade, whose read API (`get`/`query`/`contains`) cannot hand out `&mut C` (the
;; `ReadQuery` bound and the absence of a raw handle enforce it at compile time).
;; So the raw-borrow hazard physically lives in exactly one crate, and that is all
;; this check needs to scan.
(def default-root "musce_core/src")

;; A borrow carrying this marker is a deliberate, reviewed raw write (e.g. `modify`
;; itself, the derived `RelSources` reverse-index maintenance, or a test that
;; proves the footgun). The marker is greppable, so every raw borrow in the engine
;; is auditable in one search. It counts on the borrow line or an immediately
;; adjacent one, because a hand-written waiver reads best directly above the
;; statement while `rustfmt` reparks a trailing waiver onto the line just below.
(def waiver "hygiene:allow-raw-mut")

;; Mutable component borrows off the raw hecs handle. These are specific enough to
;; avoid colliding with `Vec`/`HashMap` `get_mut`, `&mut self`, or `&mut World`:
;; the hazard is always a turbofish component borrow or a hecs mutable query.
(def patterns
  [#"\.get::<\s*&mut"          ; EntityRef::get / hecs World::get of `&mut C`
   #"query(_one)?_mut\b"       ; query_mut / query_one_mut
   #"query(_one)?::<[^>]*&mut" ; query::<(&mut C, ...)> archetypal mutable query
   ])

(defn- code-of
  "The code portion of a line: everything before the first `//`. Naive on `//`
  inside string literals, which never carry these patterns, so matching the code
  portion keeps doc comments and prose that merely *mention* a raw borrow from
  tripping the guard."
  [line]
  (let [i (.indexOf line "//")]
    (if (neg? i) line (subs line 0 i))))

(defn- matches-pattern? [line]
  (let [code (code-of line)]
    (some #(re-find % code) patterns)))

(defn check-file
  "Return a seq of violation maps for one file: unwaived raw mutable borrows. A
  match is waived when the marker sits on the borrow line or an immediately
  adjacent one (see `waiver`). Unreadable files come back as no violations (the
  length gate reports skips)."
  [path]
  (try
    (let [lines (vec (fs/read-all-lines path))
          waived? (fn [i] (some (fn [j] (some-> (get lines j) (.contains waiver)))
                                [(dec i) i (inc i)]))]
      (keep-indexed
       (fn [i line]
         (when (and (matches-pattern? line) (not (waived? i)))
           {:status :fail :path (str path) :line-no (inc i) :text (str/trim line)}))
       lines))
    (catch Exception _ nil)))

(defn check-root
  "Check every `.rs` file under `root` (a dir or a single file)."
  [root]
  (let [files (if (fs/directory? root)
                (fs/glob root "**.rs")
                [(fs/path root)])]
    (mapcat check-file files)))

(defn report
  "Print one violation line, pointing at the fix (route through a mutator, or
  waive it consciously if the raw borrow is genuinely warranted)."
  [{:keys [path line-no text]}]
  (println (str "RAW-MUT: " path ":" line-no "  " text))
  (println (str "         route the write through a `World` mutator, or waive with"
                " `" waiver "` if the raw borrow is deliberate")))

(defn -main [args]
  (let [roots (if (empty? args) [default-root] args)
        viols (mapcat check-root roots)]
    (run! report viols)
    (println (str "\n" (count viols) " unwaived raw component borrow(s)"))
    (when (seq viols)
      (System/exit 1))))

;; Run the CLI only when invoked directly, not when required as a library.
(when (= *file* (System/getProperty "babashka.file"))
  (-main *command-line-args*))
