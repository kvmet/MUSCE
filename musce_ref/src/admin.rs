//! The reference game's admin (builder) verbs: the `@`-namespace, staff-gated,
//! rule-bypassing commands that compile straight to the structural action set,
//! skipping the gameplay rules a player command runs. They are game content (which
//! verbs exist, how they parse, their prose) over the engine's admin frame
//! (`CommandTable` + `Gate::Staff` + dispatch); the engine owns that mechanism.
//! See `docs/architecture/actions.md` (the three buckets) and
//! `docs/architecture/engine-and-game.md`.
//!
//! Entities are referenced by id, written `#7` (the form `@create`/`@dig` hand
//! back and a future `@find` will resolve names to). The creation verbs report the
//! new id so a builder can chain commands.

use musce_action::{Action, CommandTable, Ctx, Gate, execute};
use musce_core::{EntityId, Map, Value, World};
use musce_proto::EventKind;

/// Known `@create` kinds, listed in the error when an unknown one is asked for.
const KINDS: &str = "torch, rock, goblin, box";

/// Build the reference game's admin command table. All verbs are `Gate::Staff`;
/// only an actor carrying the `Staff` marker reaches them. `summon` is registered
/// before `set` so the `s` prefix resolves to `summon`.
pub fn commands() -> CommandTable {
    let mut t = CommandTable::new();
    t.register("tel", Gate::Staff, tel);
    t.register("goto", Gate::Staff, goto);
    t.register("summon", Gate::Staff, summon);
    t.register("create", Gate::Staff, create);
    t.register("dig", Gate::Staff, dig);
    t.register("set", Gate::Staff, set);
    t
}

// --- verbs ---------------------------------------------------------------

/// `@tel #<thing> #<dest>`: move any entity into any other. Admin: no reach or
/// takeable rule, only the engine's structural cycle check.
pub fn tel(ctx: &mut Ctx, args: &str) {
    let mut p = args.split_whitespace();
    let (Some(t_tok), Some(d_tok)) = (p.next(), p.next()) else {
        ctx.emit_self(EventKind::Feedback, "Usage: @tel #<thing> #<dest>.");
        return;
    };
    let (Some(target), Some(dest)) = (parse_ref(ctx.world, t_tok), parse_ref(ctx.world, d_tok))
    else {
        ctx.emit_self(EventKind::Feedback, bad_ref());
        return;
    };

    if execute(
        ctx.world,
        Action::Move {
            entity: target,
            into: dest,
        },
    )
    .is_err()
    {
        ctx.emit_self(
            EventKind::Feedback,
            format!("Can't put #{} there; it would create a cycle.", target.0),
        );
        return;
    }
    ctx.emit_self(
        EventKind::Feedback,
        format!("Teleported #{} into #{}.", target.0, dest.0),
    );
}

/// `@goto #<thing>`: travel to where a thing is (the room enclosing it). Refuses
/// a thing with no location (e.g. a top-level room), pointing at `@tel`.
pub fn goto(ctx: &mut Ctx, args: &str) {
    let Some(target) = parse_ref(ctx.world, args.trim()) else {
        ctx.emit_self(EventKind::Feedback, bad_ref());
        return;
    };
    let Some(room) = ctx.world.enclosing_room(target) else {
        ctx.emit_self(
            EventKind::Feedback,
            format!("#{} has no location to go to. Did you mean @tel?", target.0),
        );
        return;
    };
    if execute(
        ctx.world,
        Action::Move {
            entity: ctx.actor,
            into: room,
        },
    )
    .is_err()
    {
        ctx.emit_self(EventKind::Feedback, "Something blocks the way.");
        return;
    }
    crate::verbs::look(ctx, "");
}

/// `@summon #<thing>`: bring a thing directly to you, wherever it is now.
pub fn summon(ctx: &mut Ctx, args: &str) {
    let Some(target) = parse_ref(ctx.world, args.trim()) else {
        ctx.emit_self(EventKind::Feedback, bad_ref());
        return;
    };
    let Some(dest) = ctx.world.container_of(ctx.actor) else {
        ctx.emit_self(EventKind::Feedback, "You are nowhere to summon it to.");
        return;
    };
    if execute(
        ctx.world,
        Action::Move {
            entity: target,
            into: dest,
        },
    )
    .is_err()
    {
        ctx.emit_self(EventKind::Feedback, "You can't summon that here.");
        return;
    }
    ctx.emit_self(EventKind::Feedback, format!("Summoned #{}.", target.0));
}

/// `@create <kind>`: spawn a thing from the kind table into your room, reporting
/// its new id. Compound (Create then Move), both checks front-loaded.
pub fn create(ctx: &mut Ctx, args: &str) {
    let kind = args.trim();
    let Some(blob) = kind_blob(kind) else {
        ctx.emit_self(
            EventKind::Feedback,
            format!("Create what? Known kinds: {KINDS}."),
        );
        return;
    };
    let Some(room) = ctx.world.enclosing_room(ctx.actor) else {
        ctx.emit_self(EventKind::Feedback, "You are nowhere to create it.");
        return;
    };

    let id = match execute(ctx.world, Action::Create { components: blob }) {
        Ok(id) => id,
        Err(e) => {
            ctx.emit_self(EventKind::Feedback, format!("Couldn't create that: {e}."));
            return;
        }
    };
    if execute(
        ctx.world,
        Action::Move {
            entity: id,
            into: room,
        },
    )
    .is_err()
    {
        ctx.emit_self(EventKind::Feedback, "Created it, but couldn't place it.");
        return;
    }
    ctx.emit_self(
        EventKind::Feedback,
        format!("Created {} as #{}.", display_name(ctx.world, id), id.0),
    );
}

/// `@dig <dir> [name]`: dig a new room in a direction and link it both ways.
/// Compound; every check (direction known, no colliding exit, a room to dig from)
/// runs before the first mutation.
pub fn dig(ctx: &mut Ctx, args: &str) {
    let mut p = args.splitn(2, char::is_whitespace);
    let dir = p.next().unwrap_or("").trim();
    let Some((dir, opposite)) = opposite_dir(dir) else {
        ctx.emit_self(
            EventKind::Feedback,
            "Dig which way? Use n/s/e/w/u/d (or the full direction).",
        );
        return;
    };
    let name = p
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("a freshly dug passage");
    let Some(here) = ctx.world.enclosing_room(ctx.actor) else {
        ctx.emit_self(EventKind::Feedback, "You are nowhere to dig from.");
        return;
    };
    if has_exit(ctx.world, here, dir) {
        ctx.emit_self(
            EventKind::Feedback,
            format!("There is already an exit {dir} from here."),
        );
        return;
    }

    let new = match execute(
        ctx.world,
        Action::Create {
            components: room_blob(name),
        },
    ) {
        Ok(id) => id,
        Err(e) => {
            ctx.emit_self(EventKind::Feedback, format!("Couldn't dig: {e}."));
            return;
        }
    };
    if !dig_exit(ctx.world, here, new, dir) || !dig_exit(ctx.world, new, here, opposite) {
        ctx.emit_self(EventKind::Feedback, "Dug the room, but couldn't link it.");
        return;
    }
    ctx.emit_self(
        EventKind::Feedback,
        format!("Dug {dir} to {name} (#{}).", new.0),
    );
}

/// `@set #<id>.<component> <json>`: overwrite a whole component with a JSON value.
/// Field-level paths (`#<id>.<component>.<field>`) are reserved but not built; the
/// engine's structural guards reject the identity tag and relation tags.
pub fn set(ctx: &mut Ctx, args: &str) {
    let mut p = args.splitn(2, char::is_whitespace);
    let path = p.next().unwrap_or("").trim();
    let Some(json) = p.next().map(str::trim).filter(|s| !s.is_empty()) else {
        ctx.emit_self(
            EventKind::Feedback,
            "Usage: @set #<id>.<component> <json-value>.",
        );
        return;
    };

    let mut seg = path.split('.');
    let Some(id) = seg.next().and_then(|t| parse_ref(ctx.world, t)) else {
        ctx.emit_self(EventKind::Feedback, bad_ref());
        return;
    };
    let Some(component) = seg.next().filter(|s| !s.is_empty()) else {
        ctx.emit_self(
            EventKind::Feedback,
            "Set what? Try @set #7.description \"...\".",
        );
        return;
    };
    if seg.next().is_some() {
        ctx.emit_self(
            EventKind::Feedback,
            "Field-level @set isn't supported yet; set the whole component (@set #7.<component> <json>).",
        );
        return;
    }
    let Ok(value) = json.parse::<Value>() else {
        ctx.emit_self(
            EventKind::Feedback,
            "Value must be JSON; quote strings, e.g. @set #7.description \"a torch\".",
        );
        return;
    };

    match execute(
        ctx.world,
        Action::SetComponent {
            entity: id,
            tag: component.to_string(),
            value,
        },
    ) {
        Ok(_) => ctx.emit_self(
            EventKind::Feedback,
            format!("Set {component} on #{}.", id.0),
        ),
        Err(e) => ctx.emit_self(EventKind::Feedback, format!("Can't set that: {e}.")),
    }
}

// --- helpers -------------------------------------------------------------

/// Resolve a `#<id>` token to a live entity. `None` if it lacks the `#`, is not a
/// number, or names nothing in the world.
fn parse_ref(world: &World, token: &str) -> Option<EntityId> {
    let id = EntityId(token.strip_prefix('#')?.parse().ok()?);
    world.entity(id).is_some().then_some(id)
}

fn bad_ref() -> &'static str {
    "No such entity. Entity references look like #7."
}

/// The blob for a `@create` kind, or `None` if unknown. Plain components only (no
/// id, no relation tags), so `World::create` accepts it.
fn kind_blob(kind: &str) -> Option<Value> {
    let (marker, desc) = match kind {
        "torch" => ("item", "a guttering torch"),
        "rock" => ("item", "a heavy rock"),
        "goblin" => ("creature", "a snaggle-toothed goblin"),
        "box" => ("container", "a sturdy wooden box"),
        _ => return None,
    };
    let mut m = Map::new();
    m.insert(marker.into(), Value::Null);
    m.insert("description".into(), Value::String(desc.into()));
    Some(Value::Object(m))
}

fn room_blob(name: &str) -> Value {
    let mut m = Map::new();
    m.insert("room".into(), Value::Null);
    m.insert("description".into(), Value::String(name.into()));
    Value::Object(m)
}

/// Map a typed direction (abbreviation or full word) to its canonical name and
/// the canonical name of its opposite. Hardcoded n/s, e/w, u/d.
fn opposite_dir(d: &str) -> Option<(&'static str, &'static str)> {
    Some(match d.to_lowercase().as_str() {
        "n" | "north" => ("north", "south"),
        "s" | "south" => ("south", "north"),
        "e" | "east" => ("east", "west"),
        "w" | "west" => ("west", "east"),
        "u" | "up" => ("up", "down"),
        "d" | "down" => ("down", "up"),
        _ => return None,
    })
}

fn has_exit(world: &World, room: EntityId, dir: &str) -> bool {
    world
        .exits_of(room)
        .into_iter()
        .any(|e| world.label_of(e).as_deref() == Some(dir))
}

/// Spawn one exit entity from `from` to `to`, labeled `label`, through the
/// executor (Create the exit, then Relate it both endpoints) so the wiring
/// rides the action path like every other mutation. Returns whether it
/// committed.
fn dig_exit(world: &mut World, from: EntityId, to: EntityId, label: &str) -> bool {
    let exit = match execute(
        world,
        Action::Create {
            components: exit_blob(label),
        },
    ) {
        Ok(id) => id,
        Err(_) => return false,
    };
    execute(
        world,
        Action::Relate {
            source: exit,
            target: from,
            kind: "leads_from".into(),
        },
    )
    .is_ok()
        && execute(
            world,
            Action::Relate {
                source: exit,
                target: to,
                kind: "leads_to".into(),
            },
        )
        .is_ok()
}

fn exit_blob(label: &str) -> Value {
    let mut m = Map::new();
    m.insert("exit".into(), Value::Null);
    m.insert("label".into(), Value::String(label.to_string()));
    Value::Object(m)
}

fn display_name(world: &World, id: EntityId) -> String {
    use musce_core::Description;
    world
        .entity(id)
        .and_then(|er| er.get::<&Description>().map(|d| d.0.clone()))
        .unwrap_or_else(|| "something".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce_action::Outbound;
    use musce_core::hecs::EntityBuilder;
    use musce_core::{Creature, Description, Item, Player, Room, Staff};
    use musce_proto::{Audience, ConnectionId};

    fn spawn(w: &mut World, f: impl FnOnce(&mut EntityBuilder)) -> EntityId {
        let mut b = EntityBuilder::new();
        f(&mut b);
        w.spawn(b)
    }

    fn described(w: &mut World, marker: impl FnOnce(&mut EntityBuilder), desc: &str) -> EntityId {
        spawn(w, |b| {
            marker(b);
            b.add(Description(desc.into()));
        })
    }

    /// A world with a staff builder standing in a hall: (world, hall, builder).
    fn world_with_builder() -> (World, EntityId, EntityId) {
        let mut w = World::new();
        let hall = described(
            &mut w,
            |b| {
                b.add(Room);
            },
            "a stone hall",
        );
        let builder = spawn(&mut w, |b| {
            b.add(Player);
            b.add(Staff);
            b.add(Description("a builder".into()));
        });
        w.move_entity(builder, hall).unwrap();
        (w, hall, builder)
    }

    fn run(world: &mut World, actor: EntityId, f: impl FnOnce(&mut Ctx)) -> Vec<Outbound> {
        let mut out = Vec::new();
        let mut ctx = Ctx::new(world, actor, ConnectionId(1), &mut out);
        f(&mut ctx);
        out
    }

    fn feedback(out: &[Outbound]) -> Vec<String> {
        out.iter()
            .filter(|o| matches!(o.event.to, Audience::Connection(_)))
            .map(|o| o.event.text.clone())
            .collect()
    }

    fn re(id: EntityId) -> String {
        format!("#{}", id.0)
    }

    /// The destination of a room's exit in a given direction, if any.
    fn exit_to(w: &World, room: EntityId, dir: &str) -> Option<EntityId> {
        w.exits_of(room)
            .into_iter()
            .find(|&e| w.label_of(e).as_deref() == Some(dir))
            .and_then(|e| w.exit_destination(e))
    }

    #[test]
    fn tel_moves_any_entity_into_any_other() {
        let (mut w, hall, builder) = world_with_builder();
        let coin = described(
            &mut w,
            |b| {
                b.add(Item);
            },
            "a coin",
        ); // location-less

        let out = run(&mut w, builder, |c| {
            tel(c, &format!("{} {}", re(coin), re(hall)))
        });

        assert_eq!(w.container_of(coin), Some(hall));
        assert!(feedback(&out).iter().any(|t| t.contains("Teleported")));
    }

    #[test]
    fn tel_without_hash_prefix_is_a_bad_ref() {
        let (mut w, hall, builder) = world_with_builder();
        let out = run(&mut w, builder, |c| tel(c, &format!("7 {}", hall.0)));
        assert!(feedback(&out).iter().any(|t| t.contains("look like #7")));
    }

    #[test]
    fn goto_travels_to_the_room_a_thing_is_in() {
        let (mut w, _hall, builder) = world_with_builder();
        let cellar = described(
            &mut w,
            |b| {
                b.add(Room);
            },
            "a damp cellar",
        );
        let lamp = described(
            &mut w,
            |b| {
                b.add(Item);
            },
            "a lamp",
        );
        w.move_entity(lamp, cellar).unwrap();

        run(&mut w, builder, |c| goto(c, &re(lamp)));

        assert_eq!(w.enclosing_room(builder), Some(cellar));
    }

    #[test]
    fn goto_refuses_a_thing_with_no_location() {
        let (mut w, hall, builder) = world_with_builder();
        let void = described(
            &mut w,
            |b| {
                b.add(Room);
            },
            "a void",
        ); // top-level room

        let out = run(&mut w, builder, |c| goto(c, &re(void)));

        assert_eq!(w.enclosing_room(builder), Some(hall)); // did not move
        assert!(
            feedback(&out)
                .iter()
                .any(|t| t.contains("no location to go to") && t.contains("@tel"))
        );
    }

    #[test]
    fn summon_brings_a_thing_to_you_from_anywhere() {
        let (mut w, hall, builder) = world_with_builder();
        let far = described(
            &mut w,
            |b| {
                b.add(Room);
            },
            "a far room",
        );
        let goblin = described(
            &mut w,
            |b| {
                b.add(Creature);
            },
            "a goblin",
        );
        w.move_entity(goblin, far).unwrap();

        run(&mut w, builder, |c| summon(c, &re(goblin)));

        // It came to the builder's own container, wherever it was before.
        assert_eq!(w.container_of(goblin), Some(hall));
    }

    #[test]
    fn create_spawns_into_the_room_and_reports_the_id() {
        let (mut w, hall, builder) = world_with_builder();
        let before = w.contents(hall).len();

        let out = run(&mut w, builder, |c| create(c, "torch"));

        let contents = w.contents(hall);
        assert_eq!(contents.len(), before + 1);
        let torch = *contents.iter().find(|&&e| e != builder).unwrap();
        assert!(w.has::<Item>(torch));
        assert!(
            feedback(&out)
                .iter()
                .any(|t| t.contains("Created") && t.contains('#'))
        );
    }

    #[test]
    fn create_unknown_kind_spawns_nothing() {
        let (mut w, _hall, builder) = world_with_builder();
        let before = w.index().len();

        let out = run(&mut w, builder, |c| create(c, "dragon"));

        assert_eq!(w.index().len(), before);
        assert!(feedback(&out).iter().any(|t| t.contains("Known kinds")));
    }

    #[test]
    fn dig_creates_a_room_with_reciprocal_exits() {
        let (mut w, hall, builder) = world_with_builder();

        run(&mut w, builder, |c| dig(c, "north a winding stair"));

        let new = exit_to(&w, hall, "north").expect("north exit added to here");
        assert!(w.has::<Room>(new));
        assert_eq!(exit_to(&w, new, "south"), Some(hall)); // reciprocal back
    }

    #[test]
    fn dig_refuses_a_colliding_exit_and_creates_nothing() {
        let (mut w, _hall, builder) = world_with_builder();
        run(&mut w, builder, |c| dig(c, "north")); // first dig: succeeds
        let count = w.index().len();

        let out = run(&mut w, builder, |c| dig(c, "north")); // collides

        // The collision check fires before Create, so nothing new spawned.
        assert_eq!(w.index().len(), count);
        assert!(feedback(&out).iter().any(|t| t.contains("already an exit")));
    }

    #[test]
    fn set_overwrites_a_whole_component() {
        let (mut w, hall, builder) = world_with_builder();
        let gem = described(
            &mut w,
            |b| {
                b.add(Item);
            },
            "a plain stone",
        );
        w.move_entity(gem, hall).unwrap();

        run(&mut w, builder, |c| {
            set(c, &format!("{}.description \"a gleaming gem\"", re(gem)))
        });

        assert_eq!(
            w.component_value(gem, "description"),
            Some(Value::String("a gleaming gem".into()))
        );
    }

    #[test]
    fn set_reserves_but_rejects_field_paths() {
        let (mut w, _hall, builder) = world_with_builder();
        let out = run(&mut w, builder, |c| {
            set(c, &format!("{}.description.value \"x\"", re(builder)))
        });
        assert!(feedback(&out).iter().any(|t| t.contains("Field-level")));
        // The component was left untouched.
        assert_eq!(
            w.component_value(builder, "description"),
            Some(Value::String("a builder".into()))
        );
    }

    #[test]
    fn set_defers_to_the_engine_guards_for_id_and_relation_tags() {
        let (mut w, _hall, builder) = world_with_builder();
        let id_out = run(&mut w, builder, |c| {
            set(c, &format!("{}.id 5", re(builder)))
        });
        assert!(feedback(&id_out).iter().any(|t| t.contains("Can't set")));

        let rel_out = run(&mut w, builder, |c| {
            set(c, &format!("{}.contained_by 1", re(builder)))
        });
        assert!(feedback(&rel_out).iter().any(|t| t.contains("Can't set")));
    }
}
