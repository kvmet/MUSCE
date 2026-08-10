# MUSCE web client

A thin, pointing client graybox: a containment tree, an offers panel driven by
app-defined canonical partial groundings, typed missing-input picks, and a command
bar, all in semantic DOM. By default it connects to the server's
WebSocket (`ws://<host>:4001`, overridable with `VITE_WS_URL`) and drives the same
command stream as the text MUD.

Append `?mock` to the URL to run an in-browser stand-in with no server, useful for
UI work offline.

## Stack

Svelte 5 + Vite + TypeScript. Semantic HTML only (a `role="log"` live region, not a
terminal emulator), light/dark aware, single-column on narrow viewports. No
component library or PWA plumbing yet: those land when the UI needs them.

The wire types in `src/lib/bindings/` are generated from `musce_proto` with
`ts-rs`, so the client and server envelopes cannot drift. Regenerate them from the
repo root when the protocol changes:

```sh
TS_RS_EXPORT_DIR="$PWD/webclient/src/lib/bindings" cargo test -p musce_proto --features ts
```

`TS_RS_EXPORT_DIR` is resolved relative to the crate, so pass an absolute path.

## Setup (you run these; I don't touch the package manifest)

```sh
cd webclient
npm install         # first time
npx vite            # dev server; open the printed URL
```

`npx vite build` produces a static bundle; `npm run check` type-checks.

## What it demonstrates

- Click an entity → its app-selected offers, each `Available`, greyed `Vetoed`
  (with the reason), or `Needs` one or more named typed inputs.
- Click the brass drone → `give` asks for its `item` input using the app-supplied
  inventory candidates → pick the coin → the canonical action performs.
- Click the chest → `give` is greyed with the canonical recipient-guard reason.
- The command bar (`look`) shares the same log, proving the text path coexists
  with clicking. That coexistence is the accessibility guarantee.

## Next

- Real detail rendering: the snapshot's passive detail bag is carried but not yet
  surfaced, so a room shows its kinds rather than its description.
- Optimistic/pushed state deltas instead of the re-read after each act.
