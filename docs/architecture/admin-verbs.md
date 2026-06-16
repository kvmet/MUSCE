# Admin verbs and the reflection layer

> Status: **built.** The structural action set and the reflection operation it
> needs exist in `musce_core`/`musce_action` (engine); the admin builder verbs
> (`@tel`/`@goto`/`@summon`/`@create`/`@dig`/`@set`) are built in `musce_ref` over
> the engine's admin frame (a `Gate::Staff` `CommandTable` reached through the
> `@`-namespace). `@destroy` (pending the `@destroy`/`@purge` question below) and
> dynamic possession (`@possess`) remain deferred.

This is the building half of the action layer: the admin/builder `@`-verbs and the
type-erased reflection primitives they compile to. The core executor, the action
vocabulary, command dispatch, and atomicity live in [actions.md](actions.md); the
admin verbs are sugar over that executor, just permission-gated and routed through
the `@`-namespace (see actions.md's "Dispatch" and "Three buckets" sections).

## The admin verbs

The admin frame is a staff-gated `CommandTable` reached through the `@`-namespace.
Frame selection and the gate are engine mechanism (the floor's lifecycle verbs vs
the admin table vs the embodiment frame; `Gate::Staff` checks a `Staff` marker on
the actor); the verbs themselves are game content in `musce_ref`. Each compiles
straight to the structural action set, skipping the gameplay rules a player command
runs (see the sugar table in [actions.md](actions.md) for the per-verb action):

- `@tel #<thing> #<dest>`, `@goto #<thing>`, `@summon #<thing>` move entities.
  `@goto` travels to a thing's enclosing room (refusing a location-less target,
  pointing at `@tel`); `@summon` brings a thing to you regardless of where it is.
- `@create <kind>` spawns from a kind table into your room; `@dig <dir> [name]`
  digs a room and links it both ways (hardcoded n/s, e/w, u/d). Both report the new
  entity's id.
- `@set #<id>.<component> <json>` overwrites a whole component.

Entities are referenced by id, written `#7`; the creation verbs report the new id
so a builder can chain commands, and a future `@find` will resolve names to ids.
`@destroy` and dynamic possession (`@possess`) are deferred (see the open questions
below and [networking-and-sessions.md](networking-and-sessions.md)).

## SetComponent granularity

Components are freely mutable. The whole-component behavior is a property of the
generic admin path, not the data.

- **Typed code mutates fields in place**: `world.get::<&mut Stats>(e)?.str += 1`.
  Fully granular, the normal gameplay path.
- **`SetComponent` is type-erased**, so it works at whole-component granularity:
  it receives a tag plus a JSON value, with no compile-time knowledge of fields,
  and deserializes-and-overwrites the whole component via the `ComponentRegistry`
  (the same registry that drives persistence is the reflection layer). A JSON
  merge-patch (`@set e stats {"str": 12}`) gives field-level editing as a
  read-modify-write: serialize the current component, patch the key, deserialize,
  overwrite. Reaching one field generically without this would need a
  reflection/path system, which the JSON layer makes unnecessary.

Implementation implications, grounded in `component.rs`:

- The registry today does serialize-entity and deserialize-into-`EntityBuilder`
  (spawn/load). A live `SetComponent` needs a third per-tag function:
  deserialize-and-`insert_one` into an existing entity. Merge needs a per-tag
  serialize-one-component-to-`Value` (exposed as `World::component_value`), so the
  command layer reads the current component, patches the key, and overwrites; the
  engine owns neither the merge nor the verb. Both are small extensions of the
  existing `ser_one`/`deser_one` pattern.
- `SetComponent` must **refuse relation forward-links** and the **identity tag**.
  Writing a forward-link directly bypasses the cycle check and the reverse-index
  bookkeeping; `Id` must stay in lockstep with the `EntityIndex`. Relation tags are
  registered via `register_relation`, so the registry recognizes and rejects them,
  directing the change to `Move`/`Relate`; the generic setter is for plain-data
  components only. Load is exempt from the relation guard because
  `rebuild_relations` reconstructs the reverse index after it, whereas a live
  mutation has no rebuild pass.

The `@set` verb surface (game-side, in `musce_ref`) addresses this with a dotted
path, `@set #<id>.<component>[.<field>] <json>`:

- **`@set #7.description "a torch"`** sets the *whole* component, the direct
  `SetComponent` overwrite. This is the only form built so far, and it is enough
  for every component that exists today.
- **`@set #7.stats.str 12`**, the field form, is **reserved but not built**: a
  third path segment reports "field-level @set isn't supported yet." When built it
  is the read-modify-write above (`component_value` -> patch the key ->
  `SetComponent`).

The gate for the field form is whether the component **serializes as a JSON
object**, not its field count. A newtype like `Description(String)` serializes
*transparently* as a bare string, so `#7.description` is already the leaf: it has
no addressable sub-field and is always whole-set. A struct that serializes as
`{"key": ...}` (even a single-keyed one) does have an addressable field. Today
every component is a newtype scalar, an array (`exits`), or a unit marker; none
serialize as objects, so there is nothing to field-address yet. The first
object-shaped component (a `Stats`) is what unlocks the field form, additively on
this same syntax.

## The structural action set and reflection primitives

`Action` grows from `Move` to the full bucket-1 set, the typed reflection of the
`World` mutators: `Create { components }`, `Destroy { entity }`,
`SetComponent { entity, tag, value }`, `RemoveComponent { entity, tag }`. The
payloads are type-erased JSON; `musce_core` re-exports `serde_json`'s `Value`/`Map`
so the action layer names them without a `serde_json` dependency of its own.

Each action is a thin `execute` arm over a `World` method, the way `Move` wraps
`move_entity`: the mutation needs the private registry and ecs, so it lives in
`musce_core`.

- `World::create(&Value) -> EntityId` builds an entity from a tag->value blob and
  `spawn`s it. It is **location-less**: it makes a root entity and never places it.
  Placement is a separate `Move` the command layer composes only when it makes
  sense; an entity may legitimately stay location-less, or its container may be
  unknown at creation. Prescribing the move in the primitive would be wrong.
- `World::set_component` / `remove_component` deserialize-and-`insert_one` / remove
  one component on a live entity.
- `World::component_value(id, tag) -> Option<Value>` reads one component back as
  JSON. This is the read half of merge-patch (see "SetComponent granularity"
  above); the engine implements neither the merge nor the verb.

`execute` returns the action's **subject** `EntityId` (`Result<EntityId,
ExecError>`, widening `Move`'s shipped `Result<()>`). `Create` allocates its id
inside `spawn`, so returning it is the only way the caller learns the new id;
returning the subject uniformly keeps the other arms consistent.

Guards, enforced structurally (an `ExecError`, never player-facing):

- **Relation tags are refused** on the live paths (`create`/`set`/`remove`):
  writing a forward-link raw skips the cycle check and the reverse-index
  bookkeeping, so the change must go through `Move`/`Relate`. Load is exempt
  because `rebuild_relations` runs after it; a live mutation has no rebuild pass.
- **The identity tag is refused** on `set`/`remove`: `Id` must track the
  `EntityIndex`.
- Otherwise the usual structural checks: the entity exists, the tag is registered,
  the value deserializes.

`ComponentRegistry` gains, per registered tag, three small extensions of the
existing `ser_one`/`deser_one` pattern (deserialize-and-`insert_one` into a live
entity, remove-by-tag, serialize-one-to-`Value`) plus an `is_relation_tag`
predicate for the guard above.

## Open questions

- **`@destroy` vs `@purge`.** `despawn` reparents contents up (Reparent cascade in
  `containment.rs`), so `@destroy bag` spills its contents to the floor. Builders
  often expect destroy to take the contents with it. Decide whether `@destroy` is
  despawn-with-reparent plus a separate recursive `@purge`, or recursive by
  default. **`@destroy` is deferred out of the first admin slice pending this
  decision;** every other builder verb shipped.
- **`@dig` opposite-direction convention.** **Decided for the first admin slice:**
  hardcoded opposites n/s, e/w, u/d in `musce_ref`. A per-dig override (`@dig n=s`)
  and a content table remain a later option, added when a builder needs an
  asymmetric link.

Prior art: Bevy/flecs command buffers (the mutator set at the engine layer);
MOO/Diku `@`-commands (the admin-verb bucket). Mirror the Diku surface builders
know, but resolve it to this action set over composable components plus the JSON
registry, not Diku's fixed struct fields.
