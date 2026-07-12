# Command Dispatch and Output

> Status: **built.** The command/action boundary, the `CommandTable` registry and
> its prefix lookup, and the sim-side `Event` output channel with audience
> resolution all ship in `musce_action` (the shared `Command`/`Event` vocabulary in
> `musce_proto`). This document covers the input edge (a `Command` becoming a verb
> call) and the output edge (verbs emitting `Event`s); the action vocabulary and
> the executor those verbs drive are in [actions.md](actions.md).

## Command vs Action

The thread boundary is unchanged from `concurrency.md`: the net thread speaks
`Command` in, `Event` out. `Action` is internal to the sim and never crosses the
channel.

- A **Command** is a request with provenance. It may be rejected.
- An **Action** is the authorized, validated mutation.

The parser's whole job is `Command -> Action` (or a `Rejection`, rendered as a
`Feedback` event). Two distinct error channels: a handler's pre-commit rule check
produces a player-facing `Rejection`; `execute` produces a structural `ExecError`,
which a correct handler has already ruled out, so it signals a bug rather than
ordinary play.

Scripted behavior reaches the same rules by going through the same verb helpers a
player command does, not by emitting raw actions. A sequence step references a
**verb/intent**, not a bare `Action`, so a scripted NPC walking into a now-locked
door fails exactly as a player would; a raw `Action` would skip the gameplay rule
and is reserved for the rule-bypassing admin path. (See `sequences.md`.)

## Dispatch: a command table the runtime invokes

The parser is a **registry**, not one growing `match`. Verbs register into a
command table keyed by name, looked up by longest matching prefix so
abbreviations fall out for free (`n` → `north`, `inv` → `inventory`). Each entry
is a small parse function plus its permission gate; verbs group by module
(movement, combat, communication, building) and register themselves, so adding a
verb is a local change, not an edit to a central switch. Lookup is O(verb length)
and stays flat from fifty verbs to thousands.

Two things keep a large command surface cheap:

- **N verbs are not N mutation paths.** Most verbs are thin parse functions over
  the tiny action set (the sugar table in [actions.md](actions.md)):
  `take`/`drop`/`give`/`put`/`@tel`/`@goto`/`@summon` are all one `Move` with a
  different computed destination and rule. What grows with the game is parse
  rules, not the executor, which stays small and central.
- **Dispatch is a library layer the runtime invokes, not part of it.** The sim
  thread (`musce_host`) drains the inbox and calls one dispatch entry point with
  the world and an event sink; the command table, parse rules, and `execute` live
  in the action layer. The runtime holds no command knowledge.

Which table a command hits is the active input-stack frame (see
[networking-and-sessions.md](networking-and-sessions.md)): the `@`-namespace
routes to the account/admin table, bare commands to the active in-game frame.

## Output is the Event channel, not an action

Communication mutates nothing, so it is not in the action vocabulary. The
primitive is the **Event**: the output side of the commands-in / events-out
boundary, addressed and typed.

```
Event { to: Audience, kind: EventKind, .. }
Audience  = Locus(id) | Entity(id) | Connection(id)
EventKind = System | Feedback | Narration
```

`EventKind` is a **closed set of engine-intrinsic delivery tiers** and stays that
way: `System` (out-of-band server messages: the connect banner, shutdown),
`Feedback` (a solicited reply to the actor's own command, including the
dispatcher's rejections), and `Narration` (in-world description). These are what
engine mechanism itself emits, so the engine owns the set and a game never extends
the enum.

Game presentation *channels* (speech vs emote vs a combat log vs a whisper the
client styles apart) are a different axis and ride an **additive, opaque `channel`
tag**, never new `EventKind` variants. This is deliberate and is the one rule that
keeps the protocol stable: a new variant is a wire-format and exhaustive-match
change (a migration), while an optional `channel` field defaults on old data (an
addition), so the open axis must never be enum cardinality. The field is unbuilt
until a client actually styles channels apart; today `say` emits `Narration` with
no channel, which is correct until then. When the need lands, add the field and a
`"speech"` value, touching no existing variant.

- Showing text to a player is just emitting an Event addressed to them
  (`to: Entity(player), kind: Narration`). No actor, no action. **Audience
  resolution is sim-side:** turning `Locus`/`Entity` into the connections that
  should see it needs world state (who is in the locus) and the
  connection-to-entity map, so the sim expands those audiences into
  `Connection`-addressed events before output reaches net. Net is a pure
  `Connection` pipe and never resolves audiences. (See
  [networking-and-sessions.md](networking-and-sessions.md).)
- `Say`/`Emote`/`look` are commands whose handlers emit Events and mutate nothing.
  Just as take/drop/give collapse to one `Move`, speech/emote/narrate collapse to
  one emit. The difference: `Move` is an action (it mutates) and emit is not (it
  reports).
- Gating (a silence effect blocks speech) is a rule check in the Say command
  handler, where a take's reachability check also lives, not a property of an
  action.
- An NPC overhearing speech is the perception layer reading Events off the bus
  (deferred sense-propagation), not a dependency on speech being an action.

So mutation funnels through `execute` (which emits nothing); output flows out as
Events from the verb and command handlers; the sim resolves their audiences to
connections on the way out. The same path carries a `System`'s output and a
reaction's narration (see [actions.md](actions.md)).
