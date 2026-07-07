---
name: fleet-review
description: Multi-agent code review fleet. Generates review lenses tailored to the change, sizes the fleet to the scope, spawns lens-specialized sub-agents, dedupes findings, validates adversarially, produces a grounded report. Two modes: branch (current vs main) and feature (directed at a feature or module).
---

# /fleet-review — Multi-Agent Code Review

Routes by first argument:

- `branch [base]` → [Branch review](#branch-mode) (default if no args; base defaults to `main`)
- `feature <signal>` → [Feature review](#feature-mode) (signal: file list, glob, symbol name, directory, or prose description)

Optional flag: `--lenses a,b,c` to pin lenses. Pinned lenses always run; the lens planner writes their descriptions and fills any remaining fleet budget only if coverage demands it.

Stages 3–6 are identical across modes. Only scope assembly (stage 1) and prompt framing (stage 2) differ.

**Companion files** (read when referenced):
- `lenses.md` — lens design rules and fleet sizing bands
- `prompts.md` — sub-agent prompt templates and finding schema
- `report.md` — final report template

**Operating rules**
- Read-only review of repo files. The orchestrator writes artifacts under `.fleet-review/run-<timestamp>/` and nowhere else. Suggest the user add `.fleet-review/` to `.gitignore` if not already present.
- Each stage persists its output to the run directory (`scope/`, `findings-raw.json`, `findings-deduped.json`, `findings-validated.json`, `findings-refuted.json`, `nearby-observations.json`, `report.md`, `report.json`). Keeps the orchestrator context lean and leaves a debug trail.
- Announce the run directory to the user at the start of the run.
- If any stage fails catastrophically (all lens agents malformed, scope empty, etc.), stop and report the run directory path to the user.
- Never skip the feature-mode confirmation gate.

---

## Stage 0 — Triage (branch mode, large diffs only)

Skip this stage unless the diff exceeds 5000 lines or 100 files. For normal branch reviews go straight to Stage 1.

Goal: classify changed files into review-worthiness buckets so the fleet only runs on parts that warrant it. Saves agents on wholesale deletions, renames, and mechanical edits.

1. Compute partition inputs cheaply with git (no agent needed for this part):
   - `git diff --stat <base>...HEAD` for per-file line counts
   - `git diff --diff-filter=D --name-only <base>...HEAD` for fully deleted files
   - `git diff --diff-filter=A --name-only <base>...HEAD` for fully added files
2. Spawn one or two `triage_scout` agents in parallel (split file list in half if >150 files). Each receives a file slice plus the stat data and classifies into:
   - `pure_deletion` — file removed wholesale. No review needed beyond confirming nothing imports it.
   - `mechanical` — rename, import shuffle, formatting, vendored update. Skim only.
   - `substantive` — new logic, behavioral edits, kept-and-modified files. Full fleet.
   - `config_build` — pyproject, CI, settings, makefiles. Light review by config-aware lens.
   - `unclear` — scout could not decide. Default to substantive unless user trims.
3. Write `scope/triage.json` and `scope/triage.md` (human-readable bucket summary with file counts and line counts per bucket).
4. **Confirmation gate.** Present bucket summary to the user. They pick which buckets get reviewed and at what depth. Wait for explicit approval.
5. The confirmed substantive (and optionally config_build) bucket becomes the input to Stage 1 manifest assembly. The original chunking rule still applies within that reduced set.

---

## Stage 1 — Scope

Produce scope artifacts at `.fleet-review/run-<id>/scope/`, shared identically across all lens agents. Determinism matters: every agent gets the same manifest, so finding divergence is attributable to lens, not luck.

Artifacts written at this stage:
- `scope/manifest.md` — ordered list of in-scope files, each with `kind` and a one-line reason. Lens agents must Read files from this list; files outside are off-limits.
- `scope/diff.patch` — branch mode only; the unified diff.
- `scope/intent.md` — PR description, commit messages (branch mode), or the user's feature signal plus discovery agent notes (feature mode).

### Branch mode

1. Resolve base branch. Default `main` unless the user supplied one as the second argument.
2. Write `scope/diff.patch` from `git diff <base>...HEAD`.
3. Write `scope/intent.md` with the PR description (via `gh pr view` if a PR exists) or `git log <base>..HEAD` for commit messages.
4. Build the manifest. Include:
   - Every file appearing in the diff (`kind: changed`)
   - Files containing depth-1 callers of changed symbols (`kind: caller`)
   - Tests referencing any changed symbol (`kind: test`)
5. Exclude from the manifest: generated files, vendored code, lockfiles, binaries. Check `.gitattributes` for `linguist-generated` when unsure.
6. Write `scope/manifest.md` with one entry per file: `path`, `kind`, one-line reason.
7. **Chunking rule:** if total diff lines exceed 2000, partition the manifest into chunks by shared parent directory. Any single file with >500 changed lines becomes its own chunk. Each chunk runs the full fleet independently; dedupe joins them at stage 4.

### Feature mode

The user's signal may be file list, glob, symbol name, directory, or prose description.

1. **Discovery pass.** Spawn one Agent using the `discovery` prompt from `prompts.md`, passing the user's signal. It returns a file list grouped into `core_files`, `test_files`, `integration_files`.
2. **Confirmation gate.** Present the discovered file set to the user with per-file rationale. Wait for explicit approval. The user may add, remove, or accept. Do not proceed without confirmation.
3. Write `scope/intent.md` with the confirmed user signal and the discovery agent's interpretation and notes.
4. Write `scope/manifest.md` from the confirmed file set, with `kind`: `core` | `test` | `integration`. Include one-hop integration points and any data model / schema files the feature touches.
5. **Chunking rule:** if the confirmed manifest exceeds 800 lines of reviewable code, partition by shared parent directory. Any single file >500 lines becomes its own chunk.

---

## Stage 2 — Plan the fleet, then fan out

### 2a — Fleet plan

The lens set is generated per-review from the scope, not picked from a catalog.

1. Compute the fleet band from the sizing table in `lenses.md` (reviewable size ×
   mode, with the risk bump/drop adjustments).
2. Spawn one `lens_planner` agent (template in `prompts.md`), passing the full text
   of `lenses.md` as `{lens_rules}`, the computed band as `{fleet_band}`, any
   `--lenses` values as `{pinned_lenses}`, plus the manifest, change context, and
   intent. It returns the lens set: `{name, description, persona, rationale}` per
   lens, plus `coverage_notes`.
3. Sanity-check the plan: the mandatory `correctness` lens is present, every pinned
   lens is present, the count is within band (or the overflow is justified), and no
   two lenses share a primary concern. On violation, re-prompt the planner once with
   the specific problem; on second failure, fall back to a minimal fleet
   (`correctness` plus any pinned lenses) and note the fallback in the report.
4. Write the plan to `scope/lenses.json` and show the user a one-line-per-lens
   summary (name, persona, rationale) before fanning out. This is informational, not
   a confirmation gate.
5. Chunked reviews (stage 1 chunking rule): the planner runs once on the full
   manifest and the same lens set is used for every chunk, so dedupe at stage 4 can
   join findings across chunks.

### 2b — Fan out

Spawn lens agents in parallel using the Agent tool. Each agent is independent. No shared scratchpad.

1. For each lens in the plan, spawn one Agent with:
   - `subagent_type`: `general-purpose`
   - `description`: `Fleet review — <lens>`
   - `prompt`: the `lens_agent` template from `prompts.md` with `{lens}`, `{lens_description}`, `{persona}` (all from the plan), `{mode}`, `{manifest}`, `{diff_or_feature_summary}`, `{intent}` substituted
2. Send all spawn tool calls in a **single message** so they run in parallel.

**Branch mode:** include the `nearby_observations_block` from `prompts.md` in each lens agent prompt. Findings concern the diff only; broader patterns go in `nearby_observations`. `{diff_or_feature_summary}` = embedded diff summary (not the full patch — just changed symbols + hunk count).

**Feature mode:** omit the `nearby_observations_block`. The entire confirmed file set is in-scope. `{diff_or_feature_summary}` = the feature signal and interpretation from discovery.

---

## Stage 3 — Collect

For each returned agent report:

1. Parse the JSON. On parse failure, re-prompt that agent once with the explicit schema. Drop on second failure.
2. Validate each finding against the schema in `prompts.md` (Finding Schema section). Drop findings missing required fields.
3. **Hallucination check.** Collapse runs of whitespace in both the `evidence_quote` and the target file contents, then search for the quote in the file. If the quote is not found anywhere, drop the finding. If found at different lines than cited, adjust `line_start`/`line_end` to the match and tag the finding with `line_adjusted: true`.
4. Assign each surviving finding an `id` (UUID) and tag with `agent_id` and `lens`.
5. Normalize paths (relative to repo root).

Write surviving findings to `findings-raw.json`. Collect `nearby_observations` to `nearby-observations.json` (branch mode only).

---

## Stage 4 — Dedupe

Goal: collapse same-issue findings while preserving cross-agent consensus.

1. **Structural clustering.** Group findings where all match:
   - Same `file`
   - Overlapping `line_start..line_end` ranges (any overlap)
   - Same or related `category` (taxonomy in `prompts.md`)
2. **LLM merge within cluster.** For each cluster with >1 finding, call the `dedupe_merge` prompt from `prompts.md`. It returns either a merged finding (max severity, max confidence, union of `assumes[]`, clearest `claim`, list of distinct `agent_id`s as `consensus`) or a split decision.
3. Compute `consensus_count` per surviving finding = count of distinct agents that flagged it.
4. Write the deduped set to `findings-deduped.json`.

Skip cross-location semantic linking for v1.

---

## Stage 5 — Validate

Each surviving finding gets an adversarial validator. Validators default to refuting, not confirming.

1. For each finding, spawn an Agent with:
   - `subagent_type`: `general-purpose`
   - `description`: `Validate — <title>`
   - `prompt`: the `validator` template from `prompts.md` with the finding JSON embedded
2. Batch up to 10 validator spawns per message for parallelism.
3. Each validator returns `{verdict, reasoning, evidence_quote, failed_assumptions, mitigations_found}`.
4. Apply verdicts:
   - `confirmed` → keep, annotate with validator output
   - `refuted` → move to refuted appendix (not dropped silently)
   - `unclear` → keep, flag for human review in the report
5. For surviving `critical` and `high` severity findings, run a second **independent** validator pass. Independent means a fresh Agent whose prompt contains none of the first validator's output (no verdict, reasoning, or quotes). If the two verdicts disagree, mark the finding `unclear`.
6. Write the validated set (confirmed + unclear, with verdicts attached) to `findings-validated.json`. Write refuted findings to `findings-refuted.json`.

---

## Stage 6 — Report

Render using `report.md` as the template.

**Sort order:**
1. Severity (critical → info)
2. Tiebreak: `consensus_count × confidence_weight` (high=3, medium=2, low=1)

Group by file within each severity band.

**Sections:**
- Executive summary (counts by severity, top themes)
- Findings (main body)
- Meta-findings (any category with ≥3 findings whose files share a parent directory)
- Patterns worth exploring (branch mode only, from `nearby_observations` — surface-level, no severity, framed as recommendations)
- Refuted findings appendix
- Lens coverage table (lens | raised | surviving validation)

Write outputs to the run directory:
- `.fleet-review/run-<id>/report.md` — the human-readable report
- `.fleet-review/run-<id>/report.json` — sidecar: `{mode, run_id, timestamps, lenses_run[] (the plan objects from scope/lenses.json), confirmed_findings[], unclear_findings[], refuted_findings[], nearby_observations[], lens_coverage{}}`

Tell the user the final report path when done.
