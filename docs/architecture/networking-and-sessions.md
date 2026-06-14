# Networking and Sessions

> Status: **first slice built, plus the embodiment-frame stub.** The raw TCP
> line-mode transport, the transport-agnostic `Connection` abstraction, the
> commands-in/events-out pipe (`musce_net`/`musce_proto`), and the session floor
> (`@quit`/`@who`/`@help`/`@play`, auth stubbed) are implemented and wired into the
> tick loop. The dispatcher now also routes bare commands to an embodiment frame
> via a **stub** `@play` that binds a connection to an actor `EntityId` as session
> state (see [actions.md](actions.md)). WebSocket/SSH transports, char/raw
> input-mode switching, real accounts/auth, the persisted `Controls`/`Focus`
> embodiment, and modal overlays remain proposed; the rest of this document records
> that design.

## Three layers, and the thread boundary

Keep three concerns from bleeding together:

- **Transport** (TCP / WebSocket / SSH / telnet): how bytes are framed.
- **Session**: an authenticated connection (account identity, capabilities, input mode).
- **Control**: what your input actually drives (embodiment) and what's overlaid on it (modal UI).

The thread split from [concurrency.md](concurrency.md) holds: **net is a mostly-dumb pipe**, the sim owns the logic. Net turns a transport into `Command { connection_id, input }` for the sim and renders `Event`s back. The interesting routing happens sim-side where the state lives. The one thing net holds locally is **per-connection presentation state** (input mode, color/size capabilities), because it controls framing; the sim updates that via outbound events.

## Transports: one `Connection`, many backends

Every transport reduces to a bidirectional stream plus capability flags (line- vs char-capable, color, terminal size and resize events). Each transport implements a common `Connection`; the sim never knows which one a player is on, so adding transports is additive.

- **Raw TCP line-mode** — the dumb dev transport, built first to make the loop interactive. A plain client talks to it in line mode.
- **WebSocket** — first-class, for the web client. Does char- or line-mode trivially (the client chooses framing).
- **SSH** — first-class for terminal clients, preferred over telnet for the control it gives: a real PTY with raw mode, terminal size, resize events, and auth/encryption for free. Enables TUIs, in-game VI, WASD movement. (`russh` for an in-process server.)
- **Telnet** — the classic, but the cruftiest (IAC option negotiation). Optional/later behind the same abstraction.

Output renders `Event`s to the connection's format (ANSI text first). Keep events semantic where reasonable so a web client can render richly later.

## Input mode: a connection state, not a separate port

Line vs char/raw is a **switchable property of the connection, driven by the active controller**, not a second endpoint.

- Normal play is **line-mode**: the client echoes locally and sends on Enter. This matters because per-keystroke server echo is a network round-trip per character (the reason telnet line-mode exists).
- When a controller needs keystrokes (in-game VI, a WASD movement mode, a menu), it **declares it wants char/raw mode**. The sim emits a mode-change event; net flips the connection. On exit, the controller beneath asks for line-mode again.

So "real-time echo-back" is just the active controller asking for keystrokes. VI, WASD, menus, a fullscreen map all fall out of one mechanism. SSH/PTY and WebSocket support it natively; telnet via negotiation; a line-only client simply can't enter those modes (graceful degradation).

## Sessions and control

The **input stack is never empty**: its bottom frame is always the account/session floor. What can be empty is the *control (embodiment) stack* on top of it. First login = empty control stack, so only the floor is active.

Input handling, top to bottom:

1. **Modal overlay** (menu/editor) if open. Captures input, offers an exit.
2. **Embodiment** if puppeting. Bare in-game commands act through the focused entity (the `Command -> Action -> execute` path in [actions.md](actions.md)).
3. **Account/session floor**, always present once authenticated: `@quit`, `@who`, character list, `@play <char>`, `@create`, staff puppet management.

The `@`-namespace always routes to the floor regardless of what's on top; bare commands go to the active frame. So `@quit` works whether you're a fresh login, deep in a possessed drone, or mid-edit.

One sim-side **dispatcher** implements this routing and is the single entry point the tick loop calls as it drains the command inbox. The runtime hands it each `Command` plus the world and an event sink; it selects the frame, emits output events, and (for in-game frames) produces `Action`s through `execute` (see [actions.md](actions.md)). The tick loop itself holds no command knowledge.

### Two kinds of control state (different homes, different durability)

The distinction that matters: embodied control is a *fact about the world*, not UI.

- **Embodiment / possession is world state, persisted.** "C pilots R" is a `Controls` relation (one-to-many: a controlled entity has one controller; cascade is `Detach`, so a controller's death reverts the controlled entity to its own AI rather than destroying or reparenting it). A chain is just relations: C → R → D. A per-controller `Focus(EntityId)` on the world entity marks where you currently are in the chain. Both persist, so a character piloting a robot survives a reboot still piloting it.
- **Modal UI overlay is session state, ephemeral.** Menus, an editor's cursor and undo buffer, the input mode. Even in-game VI splits this way: the *file* is a world entity (persisted), the *editing session* is the overlay (ephemeral).

### Durability tiers

- **World control chain + `Focus`**: survives disconnect *and* reboot (persisted world state).
- **Session** (overlays, input mode, connection↔account, character attachments): survives disconnect (kept server-side, keyed by account), rebuilt on reboot.
- **Connection**: ephemeral.

A reboot dropping your open menu is fine; a reboot dropping the fact you're piloting the robot is not.

### The account, and the root of the control chain

The **account is not a world entity.** It is a persisted DB record (auth, owned characters, permissions, settings) plus a live session, which is the floor.

- Login establishes the session floor; no world attachment yet.
- `@play <char>` attaches the session to a character entity (a session→world `EntityId` pointer), re-established each login.
- In-world control extends from that character via `Controls`.
- On reboot, char→robot and the character's `Focus` persist; on next login only the cheap session→char attachment is rebuilt, and you resume where you left off.

### Staff multi-puppet

A session holds several character attachments (the `p1`/`p2`/... slots), each a session→entity pointer. A command prefix (`p2 say hi`) selects the slot; that character's `Focus` resolves the active driven entity. The slots are session state; everything they point at is world state. The targeted entities may live in different zones/shards, and the locator routes the resulting action accordingly.

## Build order

1. **Built.** Raw TCP line-mode transport, to make the loop interactive (feeds the command inbox; events out to the connection).
2. WebSocket + SSH behind the same `Connection` abstraction.
3. **Floor built, auth stubbed.** The session floor (`@`-commands) is wired; every connection is an anonymous guest until real auth/accounts land.
4. Embodiment: the `Controls` relation, the `Focus` component, and the `@play` flow.
   **The action layer ([actions.md](actions.md)) landed first with a stub `@play`**
   that binds a connection to an actor `EntityId` as session state
   (`musce_action::Actors`), so in-game verbs have an actor; that part is built.
   This step then replaces the pointer with the persisted `Controls`/`Focus` world
   state without touching the verb handlers, which already take the actor
   explicitly.
5. Modal overlays: menus and editors, with input-mode switching.

### What the first slice actually built

- `musce_net`: a `Connection` trait splitting any transport into line-oriented
  read/write halves plus `Capabilities`; the raw TCP impl; the per-connection
  task and event router; and the boundary vocabulary (`Command`/`Input`,
  `Outgoing`/`Event`/`Audience`/`EventKind`, `ConnectionId`). Net is the producer
  of `Command`s and consumer of `Outgoing`; it holds only per-connection
  presentation state.
- Connection lifecycle rides the command channel as `Input::Connected` /
  `Line` / `Disconnected`, so the sim has one entry point for allocating, driving,
  and tearing down a session.
- Net handles only `Audience::Connection`. Resolving `Entity`/`Room` to the
  connections that should see an event is **sim-side** (it needs world state and
  the connection-to-entity map), done by the action layer's audience resolver
  before output reaches net; net never resolves audiences and logs an error if an
  unresolved audience ever reaches it.
- A single sim-side dispatcher (`musce_host`, `dispatch.rs`) is the one entry
  point the tick loop calls as it drains the inbox; it owns the input-stack
  routing above and takes `&mut World`. The `@`-namespace and connection lifecycle
  land on the session floor (`session.rs`); a bare command routes to the
  embodiment frame, which this slice realizes as the stub actor binding plus the
  action layer's command table ([actions.md](actions.md)). The persisted
  `Controls`/`Focus` embodiment replaces the stub binding behind this same entry
  point without touching the floor or the verb handlers.

New engine pieces this needs: a `Controls` relation (a new instance of the relation layer, cascade `Detach`) and a `Focus` component. Both are small additions to `musce_core`.
