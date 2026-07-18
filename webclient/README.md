# MUSCE web client

A thin, pointing client graybox: a containment tree, an offers panel driven by the
`musce_ref::offers` semantics (the three-way status and the `NeedsRole` sub-pick),
and a command bar, all in semantic DOM. It runs against an in-browser mock
connection, so it works before the WebSocket transport exists; swapping to the
real server is one file (`src/lib/connection.ts`).

## Stack

Svelte 5 + Vite + TypeScript. Semantic HTML only (a `role="log"` live region, not a
terminal emulator), light/dark aware, single-column on narrow viewports. No
component library or PWA plumbing yet: those land when the UI needs them.

## Setup (you run these; I don't touch the package manifest)

```sh
cd webclient
npm init -y
npm install svelte
npm install -D vite @sveltejs/vite-plugin-svelte @tsconfig/svelte typescript svelte-check
npx vite            # dev server; open the printed URL
```

`npx vite build` produces a static bundle; `npx svelte-check` type-checks.

## What it demonstrates

- Click an entity → its offers, each `Available` / greyed `Vetoed` (with the
  reason) / `NeedsRole`.
- Click the chest → `put` shows `NeedsRole(object)` → pick a held item → it
  resolves to `Available` (held) and performs, or `Vetoed` with the real reason.
- Click the locked `north` gate → `go` is greyed with "It's locked."
- The command bar (`look`) shares the same log, proving the text path coexists
  with clicking. That coexistence is the accessibility guarantee.

## Next

- Replace `MockConn` with a `WsConn` once `musce_net` has the WebSocket transport
  and `musce_proto` carries the offers request/response.
- Generate `src/lib/protocol.ts` from `musce_proto` with `ts-rs` so the wire
  shapes cannot drift.
