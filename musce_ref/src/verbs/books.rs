//! Reading and writing books: the reference game's consumer of the engine's cold
//! content store. A book's heavy text does not live in the world; the resident
//! entity carries only a small `Readable` component holding a key, and the text
//! sits cold in the `kv` store under that key. `read` fetches it on demand and
//! `inscribe` overwrites it, both through the engine's cold-op channel (the sim
//! never touches the store). See `docs/architecture/persistence.md`.

use musce::action::Ctx;
use musce::wire::EventKind;
use musce::world::{EntityId, NamedComponent};
use serde::{Deserialize, Serialize};

use crate::names;

/// A hot marker that a thing can be read: it holds the cold-store key its text
/// lives under, nothing more. The text is off-heap; this stays resident so the
/// world can find the key without paying for the payload every tick.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Readable {
    pub key: String,
}

impl NamedComponent for Readable {
    const TAG: &'static str = "readable";
}

/// The cold-store key for a book entity. Entity-scoped, so it is fixed at spawn and
/// never changes: that is what lets a write be a plain overwrite with no cold-first
/// ordering (the key is set before any bytes exist, and reading an unwritten key
/// reads as blank, not as a dangling reference). Content-addressed keys with
/// cross-copy dedup are a later, separate path. See `docs/architecture/persistence.md`.
pub(crate) fn book_key(id: EntityId) -> String {
    format!("book:{}", id.0)
}

/// `read <thing>`: fetch a readable's cold text and show it. The fetch runs off the
/// sim thread, so the text arrives a beat later, delivered straight to the reader.
pub fn read(ctx: &mut Ctx, args: &str) {
    let query = args.trim();
    if query.is_empty() {
        ctx.feedback("Read what?");
        return;
    }
    let Some(target) = names::resolve_nearby(ctx.world, ctx.actor, query) else {
        ctx.feedback("You don't see that here.");
        return;
    };
    match key_of(ctx, target) {
        Some(key) => ctx.cold_read(key, EventKind::Narration),
        None => ctx.feedback("There's nothing to read on that."),
    }
}

/// `inscribe <thing> <text>`: overwrite a readable's cold text. The target is the
/// first word (a book's single-word name or alias); everything after it is the
/// text. The write is durable before the acknowledgement returns.
pub fn inscribe(ctx: &mut Ctx, args: &str) {
    let Some((name, text)) = args.trim().split_once(char::is_whitespace) else {
        ctx.feedback("Inscribe what, and with what words?");
        return;
    };
    let text = text.trim();
    if text.is_empty() {
        ctx.feedback("Write what?");
        return;
    }
    let Some(target) = names::resolve_nearby(ctx.world, ctx.actor, name) else {
        ctx.feedback("You don't see that here.");
        return;
    };
    match key_of(ctx, target) {
        Some(key) => ctx.cold_write(key, text.as_bytes().to_vec()),
        None => ctx.feedback("You can't write on that."),
    }
}

/// A readable's cold-store key, if the entity carries one.
fn key_of(ctx: &Ctx, entity: EntityId) -> Option<String> {
    ctx.world.get::<Readable>(entity).map(|r| r.key.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kinds::Item;
    use musce::action::{ColdOp, Outbound, Verdict};
    use musce::wire::ConnectionId;
    use musce::world::hecs::EntityBuilder;
    use musce::world::{Description, Locus, Name, World};

    fn spawn(w: &mut World, f: impl FnOnce(&mut EntityBuilder)) -> EntityId {
        let mut b = EntityBuilder::new();
        f(&mut b);
        w.spawn(b)
    }

    /// A room holding the actor, a readable journal (key set as at seed), and a
    /// plain rock that carries no `Readable`.
    fn fixture() -> (World, EntityId, EntityId) {
        let mut world = World::new();
        let room = spawn(&mut world, |b| {
            b.add(Locus);
            b.add(Description("a room".into()));
        });
        let actor = spawn(&mut world, |b| {
            b.add(Description("an adventurer".into()));
        });
        world.move_entity(actor, room).unwrap();

        let book = spawn(&mut world, |b| {
            b.add(Item);
            b.add(Name("a leather journal".into()));
        });
        world.insert(
            book,
            Readable {
                key: book_key(book),
            },
        );
        world.move_entity(book, room).unwrap();

        let rock = spawn(&mut world, |b| {
            b.add(Item);
            b.add(Name("a rock".into()));
        });
        world.move_entity(rock, room).unwrap();

        (world, actor, book)
    }

    /// Drive a handler and render the cold requests it queued into comparable
    /// strings. Reads `cold_ops()` before the `Ctx` is dropped.
    fn cold_ops(world: &mut World, actor: EntityId, f: impl FnOnce(&mut Ctx)) -> Vec<String> {
        let mut out: Vec<Outbound> = Vec::new();
        let verdict = Verdict::guest();
        let mut ctx = Ctx::new(world, actor, ConnectionId(1), &verdict, &mut out);
        f(&mut ctx);
        ctx.cold_ops()
            .iter()
            .map(|op| match op {
                ColdOp::Read { key, .. } => format!("read {key}"),
                ColdOp::Write { key, bytes, .. } => {
                    format!("write {key} = {}", String::from_utf8_lossy(bytes))
                }
            })
            .collect()
    }

    #[test]
    fn read_queues_a_cold_read_for_the_books_key() {
        let (mut world, actor, book) = fixture();
        let ops = cold_ops(&mut world, actor, |c| read(c, "journal"));
        assert_eq!(ops, vec![format!("read book:{}", book.0)]);
    }

    #[test]
    fn inscribe_queues_a_cold_write_of_the_text() {
        let (mut world, actor, book) = fixture();
        let ops = cold_ops(&mut world, actor, |c| inscribe(c, "journal Hello there"));
        assert_eq!(ops, vec![format!("write book:{} = Hello there", book.0)]);
    }

    #[test]
    fn reading_an_unreadable_thing_queues_nothing() {
        let (mut world, actor, _) = fixture();
        assert!(cold_ops(&mut world, actor, |c| read(c, "rock")).is_empty());
    }

    #[test]
    fn inscribe_without_words_queues_nothing() {
        let (mut world, actor, _) = fixture();
        assert!(cold_ops(&mut world, actor, |c| inscribe(c, "journal")).is_empty());
    }
}
