# Networking and Sessions

> Status: **first slice built, plus the session attachment and durable
> embodiment.** The raw TCP line-mode transport, the transport-agnostic
> `Connection` abstraction, the commands-in/events-out pipe
> (`musce_net`/`musce_proto`), and the session floor
> (`@quit`/`@who`/`@help`/`@play`, auth stubbed) are implemented and wired into the
> tick loop. The dispatcher routes bare commands to an embodiment frame through
> the connection's **session attachment**: `@play` records which *character* the
> connection drives as session state on the floor, and the driven actor is
> resolved live from that character's `Focus` (`actor =
> focus_of(character).unwrap_or(character)`); the audience resolver consumes a
> conn->actor index derived the same way (see [actions.md](actions.md)). The
> persisted `Controls` and `Focus` relations make embodiment durable: a character
> piloting a robot survives a reboot still piloting it. Dynamic possession (the
> `@possess`/`@unpossess` admin verbs) is built: staff can establish and tear down
> a `Controls` edge at runtime. WebSocket/SSH transports, char/raw input-mode
> switching, real accounts/auth, and modal overlays remain proposed; the rest of
> this document records that design.

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

- **Embodiment / possession is world state, persisted**, and it is two separate facts that are easy to conflate:
  - **`Controls` is the capability wiring**: which entities a character is plugged into and *may* drive. A relation, with **source = the controlled entity** (it has one controller) and **target = the controller** (it has many sources); `ACYCLIC` (chains, never loops); cascade `Detach`, so a controller's death reverts each controlled entity to its own AI rather than destroying or reparenting it. A chain is just edges: character → mech → drone. This is a persistent capability: it holds whether or not you are currently driving any of them.
  - **`Focus` is the cursor**: a relation whose source is the controller and whose target is the single node in that chain your keystrokes are live on *right now*. One per controller (a source has one target), stored as the forward link on the character, persisted. Absence of `Focus` means "drive yourself" (the character); a present `Focus` means you are piloting that entity. Lowering `Focus` back to yourself tears down no `Controls` edge, so you step out of the mech and back in without re-establishing control. Making it a relation rather than a lone component is what lets a focused entity's despawn clear the cursor through the ordinary `Detach` cascade: the engine tracks focus -> target directly and never has to infer the focuser from the control wiring.

  Both persist, so a character piloting a robot survives a reboot still piloting it. The distinction only becomes *visible* with nested control (a cursor needs a chain to walk) or with stepping out of a puppet while keeping the ability to re-enter; in the flat single-puppet case the two look identical, which is why they are worth naming apart before that case arrives.
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

So there are two roots of two different trees, split exactly on the world / non-world line: the **account** is the root of ownership and session (it owns characters and is not a world entity), and the **character** is the root of the control chain (the `Controls` relation between world entities, with its `Focus`). `@play` is the bridge between them, and a session may hold several control-chain roots (the `p1`/`p2` characters) while the account sits above all of them and never enters the world.

### Resolving a command to an actor

Putting the layers together, a bare command on a connection resolves to the entity it drives by walking from the ephemeral edge down into persisted world state:

```
connection  →  session  →  character  →  Focus  →  actor
(ephemeral)    (session,    (session      (world,    (the entity the
               keyed by     attachment,   persisted)  command acts through)
               account)     a slot)
```

1. **connection → session**: the live transport; re-established on every connect.
2. **session → character**: the `@play` attachment (a slot, `p1` by default). Session state: survives disconnect, rebuilt on login.
3. **character → `Focus` → actor**: `actor = focus_of(character).unwrap_or(character)`. World state, read live, so a `pilot` command that changes `Focus` redirects subsequent commands at once.

The reverse walk, from a driven puppet back up to its character, is `World::control_root` (game verbs like `pilot`/`release` act on the character, not the puppet, so they walk up from the resolved actor). The two compose safely because `set_focus` refuses a cursor outside the controller's `Controls` chain (`FocusError::NotControlled`): a character's `Focus` is always within its own control subtree, so down-then-up returns where it started. Establishing the `Controls` edge is game/admin policy; constraining where the cursor may point is structure, enforced at the single `set_focus` mutator.

The audience resolver consumes the same mapping in reverse (actor → the connections that perceive it), derived from the session attachments and `Focus`, never stored as its own truth.

**Invalidation when the puppet dies.** Destroying the focused entity commits like any other action; nothing un-commits it. Because `Focus` is a relation with the focused entity as its target, that despawn clears the focuser's cursor through the ordinary `Detach` cascade, the same machinery that reverts a dead controller's puppets: the structural reset is automatic, and the relation layer already keeps the reverse index it needs, so there is no bespoke despawn path and no inference from the `Controls` wiring. Per the standing rule that reactions respond rather than veto (see [actions.md](actions.md)), the *prose* ("your puppet collapses; you are yourself again") belongs to a reaction on that despawn; the cursor reset itself is the cascade. The reaction layer is now built: structural mutations emit typed facts (`Fact::Destroyed`) that a `System` reads from `SystemCtx::facts` (see [facts.md](facts.md)), so this collapse narration is a reaction the game can add, written exactly like the reference game's `death_cry`. The cascade keeps world state consistent regardless; a game that adds no such reaction simply leaves the player back in their own body without narration. A resolution-time guard that refuses to hand a verb a dead actor stays as a **defensive backstop only**, and logs if it ever fires: with the cascade in place a `Focus` aimed at a despawned entity means corrupt or partially loaded state, not ordinary play. It is not the mechanism, and it must not silently paper over the dangling pointer.

### Establishing control: the target design and the first slice

> Status: **built.** Both the first embodiment slice and dynamic possession ship.
> The slice provides the `pilot`/`release` game verbs in `musce_ref` over the
> `Controls`/`Focus` relations and the `Focus`-resolved actor path; the admin
> `@possess`/`@unpossess` verbs establish and tear down a `Controls` edge at
> runtime. The target design above remains the canonical end state the gameplay
> possess-gate and the `p1`/`p2` multi-puppet slots grow into.

Creating a `Controls` edge at runtime is a staff `@possess <target>` /
`@unpossess <target>` pair in the admin table (the rule-bypassing admin bucket of
[actions.md](actions.md)). `@possess` establishes the `Controls` edge **only**: it
grants the capability to pilot the target and stops there. Aiming the control
cursor is the player `pilot` verb's job, which keeps "may drive" and "is driving"
separate (the `Controls` / `Focus` split above) so possessing a thing does not
yank your keystrokes onto it. `@unpossess` drops the edge; because that can strand
a `Focus` pointing into the now-detached subtree, it first clears a focus aimed at
the target or any of its descendants, then unrelates. It is named `@unpossess`,
not `@release`, to avoid colliding with the bare `release` focus verb. A later
gameplay possession (you may pilot this *if* you hold the key) is a game verb with
a game-supplied gate, the way the takeable rule is game policy.

The reference seed keeps one pre-wired `Controls` edge (a character controlling a
drone) as starter content, so the embodiment loop runs out of the box; `@possess`
is how a builder wires the same edge onto any other target at runtime.

A **known boundary**, out of scope for this slice: possessing a character that is
itself actively piloted nests control, and the focused entity's `control_root`
relocates to wherever that inner character's own pilot writes `Focus`. The flat
case is what is built: staff drives an NPC or object not itself driving anything.

The engine/game split holds throughout: `Controls` and `Focus` are engine
primitives, the resolution path and the possession actions (`Relate`/`Unrelate`
over the `Controls` edge) are engine, and which entities are possessable, the
verbs, and any gameplay gate are game policy in the reference game.

### Staff multi-puppet

A session holds several character attachments (the `p1`/`p2`/... slots), each a session→entity pointer. A command prefix (`p2 say hi`) selects the slot; that character's `Focus` resolves the active driven entity. The slots are session state; everything they point at is world state. The targeted entities may live in different zones/shards, and the locator routes the resulting action accordingly.

## Build order

1. **Built.** Raw TCP line-mode transport, to make the loop interactive (feeds the command inbox; events out to the connection).
2. WebSocket + SSH behind the same `Connection` abstraction.
3. **Floor built, auth stubbed.** The session floor (`@`-commands) is wired; every connection is an anonymous guest until real auth/accounts land.
4. **Embodiment**, in sub-steps (the model is spelled out under "Sessions and
   control" above):
   - **Built.** The session attachment: `@play` records which actor a connection
     drives as **session state** on the floor; the audience resolver derives its
     conn->actor index from those attachments. Which actor `@play` chooses is game
     policy, injected by the `Game`'s `choose_actor` (see
     [engine-and-game.md](engine-and-game.md)); the floor (`@quit`/`@who`/`@help`)
     stays engine.
   - **Built (the first embodiment slice).** The `Controls` and `Focus` relations
     in `musce_core`; the resolution path rerouted through `Focus`
     (`actor = focus_of(character).unwrap_or(character)`, read live); a seeded
     control edge plus `pilot`/`release` game verbs in `musce_ref`; and the
     `Focus` `Detach` cascade that clears a controller's cursor when its puppet
     despawns. This makes durable embodiment real end to end without the admin
     table. See "Establishing control" and "Resolving a command to an actor"
     above.
   - **Built.** Dynamic possession (`@possess`/`@unpossess`): staff establish and
     tear down a `Controls` edge at runtime through the admin frame. `@possess`
     wires the edge only; aiming `Focus` stays with the `pilot` verb. Nested
     possession of an already-piloted character is a known unhandled boundary (see
     "Establishing control" above). **Deferred:** the gameplay possess-gate and the
     `p1`/`p2` multi-puppet slots, which back or extend the attachment without
     touching the verb handlers.
5. Modal overlays: menus and editors, with input-mode switching.

### What the first slice actually built

- `musce_net`: a `Connection` trait splitting any transport into line-oriented
  read/write halves plus `Capabilities`; the raw TCP impl; the per-connection
  task and event router; and the wire vocabulary in `musce_proto`
  (`Command`/`Input`, `Outgoing` carrying a connection-bound `Delivery`,
  `EventKind`, `ConnectionId`). That crate is the wire boundary only: it holds no
  world identity and depends on nothing, because output has been resolved to a
  connection by the time it reaches this layer. The semantic, world-addressed
  authoring form (`Event`/`Audience`) lives in `musce_action` next to the resolver.
  Net is the producer of `Command`s and consumer of `Outgoing`; it holds only
  per-connection presentation state.
- Connection lifecycle rides the command channel as `Input::Connected` /
  `Line` / `Disconnected`, so the sim has one entry point for allocating, driving,
  and tearing down a session.
- Net routes a `Delivery`, which is already bound to a single connection.
  Resolving `Entity`/`Locus` to the connections that should see an event is
  **sim-side** (it needs world state and the connection-to-entity map), done by the
  action layer's audience resolver, which produces `Delivery`s; an unresolved
  audience therefore cannot reach net by construction, rather than being caught by a
  runtime guard. The conn->actor map the resolver consumes is derived from the
  floor's session attachments, which own it.
- A single sim-side dispatcher (`musce_host`, `dispatch.rs`) is the one entry
  point the tick loop calls as it drains the inbox; it owns the input-stack
  routing above and takes `&mut World`. The `@`-namespace and connection lifecycle
  land on the session floor (`session.rs`); a bare command routes to the
  embodiment frame, which this slice realizes as the connection's session
  attachment plus the injected game's command table ([actions.md](actions.md)).
  Durable `Controls`/`Focus` embodiment will back the attachment behind this same
  entry point without touching the floor or the verb handlers.

Durable embodiment added two `musce_core` pieces, both new instances of the
relation layer with cascade `Detach`: the `Controls` relation (the capability
wiring; source the controlled entity, target the controller) and the `Focus`
relation (the cursor; source the controller, target the focused entity).
