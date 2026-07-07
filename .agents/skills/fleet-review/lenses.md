# Lens Design Guide

Lenses are generated per-review by the `lens_planner` agent (template in `prompts.md`),
not picked from a catalog. This file defines what a good lens is, how many to run, and
the rules the planner must follow. The orchestrator passes this file's rules to the
planner via its prompt template.

## What a lens is

A lens is a single review perspective defined by the failure class it hunts. A lens
is well-formed when:

- **It names a concrete failure class.** "Race conditions in the new cache layer" is
  a lens; "code quality" is not.
- **It is falsifiable per-finding.** A finding under the lens can be confirmed or
  refuted by reading code. "This feels complex" cannot; "this function mutates shared
  state without a lock" can.
- **It is scoped to what the change actually touches.** A security lens on a
  docs-only diff is padding. Every lens must cite, in its `rationale`, which manifest
  files or diff hunks make it worth running.
- **Its primary concern does not duplicate another lens in the set.** Overlap at the
  edges is fine (consensus across lenses is signal, and dedupe handles it); two
  lenses with the same primary concern is a wasted agent.

## Generating the lens set

The planner derives lenses from the scope artifacts: what languages and frameworks
appear, what the change does (from `intent.md`), what could plausibly break. The
process is subtractive, not additive: start from "every way this change could be
wrong," group into distinct failure classes, and emit one lens per class that
survives the padding test.

**Mandatory baseline:** every fleet includes a `correctness` lens (logic bugs: wrong
operator, off-by-one, inverted boolean, mis-ordered args, wrong constant). It is the
catch-all for bugs no specialized lens owns.

**Seed dimensions** (inspiration for the planner, not a menu — a generated lens
should be more specific than these, and dimensions with no purchase on the change
should produce no lens):

security, concurrency, error handling, API/contract compatibility, performance,
data integrity and persistence, test adequacy, readability and misleading names,
dependency and supply-chain risk, resource lifecycle (handles, connections,
memory), input validation at trust boundaries, domain invariants specific to
this codebase.

**Domain lenses:** when the repo has a clear domain (a game, a trading system, a
compiler), the planner should consider one or two lenses for domain-specific
invariants it can infer from the code and docs (e.g. "economy exploit surfaces,"
"parser precedence regressions"). These earn their slot the same way: only if the
change touches that surface.

## Personas

Each lens carries a short persona: a stance that shapes tone and emphasis within
the lens. The planner generates it alongside the lens. Good personas are concrete
occupants of a viewpoint ("ops engineer paged at 3am," "attacker with a copy of the
source," "new hire reading this file for the first time," "the user whose data this
migration moves"). Persona never expands the lens's scope; it only colors what the
lens emphasizes.

## Fleet sizing

Fleet size scales with reviewable size and risk, within bands. Reviewable size =
changed lines in the manifest (branch mode) or total LOC of core + integration
files (feature mode).

| Mode | Reviewable size | Lenses |
|------|-----------------|--------|
| branch | < 150 lines | 2–3 |
| branch | 150–1000 lines | 4–6 |
| branch | 1000–2000 lines | 6–8 |
| branch | > 2000 lines (chunked) | 4–6, same lens set for every chunk |
| feature | < 400 LOC | 3–4 |
| feature | 400–800 LOC | 5–7 |
| feature | > 800 LOC (chunked) | 4–6, same lens set for every chunk |

Adjustments:

- **Risk bump:** move up one band when the change touches authentication, crypto,
  schema migrations, concurrency primitives, or a public API surface.
- **Risk drop:** move down one band for changes that are docs-only, tests-only, or
  dominated by mechanical edits.
- **Never pad.** The band is a budget, not a quota. If the change only plausibly
  fails in three ways, run three lenses and say so.
- **Justified overflow:** the planner may exceed the band by at most two lenses when
  the change genuinely spans more distinct risk surfaces than the band allows, and
  must say which surfaces forced it.
