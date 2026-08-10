//! The handler context and its emit API: the engine surface an app's verb
//! handlers program against. `Ctx` carries the world a handler mutates, the actor
//! it acts through, the connection that issued the command, and the output buffer
//! it emits into. The emit methods address output semantically (first-person to
//! the actor, third-person to the locus with the actor or a set of parties
//! excluded, or directed to a specific entity); the dispatcher resolves those
//! audiences to connections
//! afterward. See
//! `docs/architecture/actions.md`.

use std::time::SystemTime;

use musce_core::{EntityId, Fact, World};
use musce_proto::{ConnectionId, EventKind, Outgoing};

use crate::audience::{Outbound, resolve};
use crate::bindings::Actors;
use crate::caps::{CapId, Verdict};
use crate::event::Event;
use crate::perform::{AffordanceRegistry, PerformError, PerformOutcome};
use crate::schema::GroundAction;

/// The acting principal a command runs under: the actor entity the connection
/// drives, the connection that issued the command, and the account-scoped
/// authorization verdict. Constructed as one value so those three inputs cannot
/// drift between dispatch and the handler context.
#[derive(Clone, Copy)]
pub struct Caller<'a> {
    actor: EntityId,
    conn: ConnectionId,
    verdict: &'a Verdict,
}

impl<'a> Caller<'a> {
    pub fn new(actor: EntityId, conn: ConnectionId, verdict: &'a Verdict) -> Self {
        Self {
            actor,
            conn,
            verdict,
        }
    }

    pub fn actor(&self) -> EntityId {
        self.actor
    }

    pub fn conn(&self) -> ConnectionId {
        self.conn
    }

    pub fn verdict(&self) -> &'a Verdict {
        self.verdict
    }
}

/// A handler's request to the cold content store ([`musce_persistence::KvStore`]),
/// recorded during a command and carried out afterward, off the sim thread. A verb
/// cannot touch the store directly (the sim holds none, and the store is async), so
/// it records the intent here exactly as it records perception output in `out`; the
/// runtime drains these and hands them to the cold task. A `Read` result is decoded
/// by the app and delivered straight to `conn`; a `Write` overwrites the key's
/// bytes and acks `conn`. See `docs/architecture/persistence.md`.
pub enum ColdOp {
    /// Fetch `key`; deliver its decoded value to `conn` rendered as `kind` (or a
    /// "nothing there" line if the key is absent).
    Read {
        key: String,
        conn: ConnectionId,
        kind: EventKind,
    },
    /// Store `bytes` under `key`, overwriting; ack `conn` on completion.
    Write {
        key: String,
        bytes: Vec<u8>,
        conn: ConnectionId,
    },
}

/// The per-command context handed to a handler: the world it mutates, the actor
/// it acts through, the connection that issued it, and the output buffer it emits
/// into. The actor is explicit so handlers are callable directly in tests and,
/// later, by AI and sequences. It also carries the caller's account-scoped verdict
/// read-only: the command table checks it before invoking a handler, and an inline
/// rule can ask the same authority without deriving it from the actor body.
///
/// The principal fields are read-only to handlers:
///
/// ```compile_fail
/// use musce_action::Ctx;
/// use musce_core::EntityId;
///
/// fn redirect(ctx: &mut Ctx<'_>) {
///     ctx.actor = EntityId(99);
/// }
/// ```
pub struct Ctx<'a> {
    pub world: &'a mut World,
    caller: Caller<'a>,
    out: &'a mut Vec<Outbound>,
    /// Cold-store requests the handler recorded. Owned (not a borrowed sink like
    /// `out`) because a cold op is self-contained and needs no world/actor
    /// resolution: the dispatcher moves this vec out after the handler and routes it
    /// to the cold task.
    cold: Vec<ColdOp>,
}

impl<'a> Ctx<'a> {
    pub fn new(world: &'a mut World, caller: Caller<'a>, out: &'a mut Vec<Outbound>) -> Self {
        Ctx {
            world,
            caller,
            out,
            cold: Vec::new(),
        }
    }

    /// The body this command acts through.
    pub fn actor(&self) -> EntityId {
        self.caller.actor()
    }

    /// The connection that issued this command.
    pub fn conn(&self) -> ConnectionId {
        self.caller.conn()
    }

    /// The account-scoped authorization this handler runs under. Its lifetime is
    /// independent of the `Ctx` borrow, so a handler may retain it while yielding
    /// the world/output pair to a shared performer.
    pub fn verdict(&self) -> &'a Verdict {
        self.caller.verdict()
    }

    /// Whether this caller may exercise `cap`. This names the authorization result,
    /// not literal membership: superuser authority also permits the capability.
    pub fn permits(&self, cap: CapId) -> bool {
        self.caller.verdict().permits(cap)
    }

    /// Whether superuser authority is in force for this command.
    pub fn is_su(&self) -> bool {
        self.caller.verdict().is_su()
    }

    /// The world and the raw output buffer together. The seam a shared app routine
    /// emits through when it has no `Ctx` in common with its other callers: the
    /// narrating perform runs from a verb handler (a `Ctx`), a click (a `Ctx`), and
    /// a tick system's driver closure (neither), so it takes these two borrows
    /// explicitly and each caller yields them from whatever context it holds. Split
    /// out as a pair because the routine mutates the world and emits at once, which a
    /// single `&mut self` accessor to either field alone could not satisfy.
    pub fn world_and_out(&mut self) -> (&mut World, &mut Vec<Outbound>) {
        (self.world, self.out)
    }

    /// Run a canonical action through the shared performer under this command's
    /// account authority. The action cannot substitute a different actor body.
    pub fn perform(
        &mut self,
        registry: &AffordanceRegistry,
        action: &GroundAction,
    ) -> Result<PerformOutcome, PerformError> {
        if action.actor() != self.actor() {
            return Err(PerformError::ActorMismatch {
                caller: self.actor(),
                action: action.actor(),
            });
        }
        let verdict = self.caller.verdict();
        registry.perform(self.world, self.out, verdict, action)
    }

    /// First-person output, straight to the acting connection.
    pub fn emit_self(&mut self, kind: EventKind, text: impl Into<String>) {
        self.out
            .push(Outbound::new(Event::to_connection(self.conn(), kind, text)));
    }

    /// Plain feedback to the acting connection. The dispatcher uses this for
    /// parse-level replies (unknown verb, gated) before any handler runs.
    pub fn feedback(&mut self, text: impl Into<String>) {
        self.emit_self(EventKind::Feedback, text);
    }

    /// Record a cold read: fetch `key` and deliver its decoded value to this
    /// command's connection, rendered as `kind`. The fetch runs off the sim thread
    /// after the handler returns; the result arrives asynchronously, so the handler
    /// emits nothing itself for the read.
    pub fn cold_read(&mut self, key: impl Into<String>, kind: EventKind) {
        self.cold.push(ColdOp::Read {
            key: key.into(),
            conn: self.conn(),
            kind,
        });
    }

    /// Record a cold write: store `bytes` under `key`, overwriting. Acked to this
    /// command's connection once durable. The app encodes `bytes`; the store keeps
    /// them opaque.
    pub fn cold_write(&mut self, key: impl Into<String>, bytes: Vec<u8>) {
        self.cold.push(ColdOp::Write {
            key: key.into(),
            bytes,
            conn: self.conn(),
        });
    }

    /// The cold-store requests recorded so far. For tests that drive a handler
    /// through a `Ctx` directly and assert what it queued.
    pub fn cold_ops(&self) -> &[ColdOp] {
        &self.cold
    }

    /// Move the recorded cold requests out, leaving the buffer empty. The dispatcher
    /// calls this once, after the handler, to route them to the cold task.
    pub(crate) fn take_cold(self) -> Vec<ColdOp> {
        self.cold
    }

    /// Directed output to a specific entity, resolved to the connection(s) driving
    /// it at output time. If the entity drives no connection it reaches no one, the
    /// same way narration to a locus of NPCs does; the in-world act still happened.
    pub fn emit_entity(&mut self, target: EntityId, kind: EventKind, text: impl Into<String>) {
        self.out
            .push(Outbound::new(Event::to_entity(target, kind, text)));
    }

    /// Third-person output to everyone in `locus` except the actor, so the actor
    /// does not see both their own first-person line and the locus's view of it.
    pub fn emit_locus_except_self(
        &mut self,
        locus: EntityId,
        kind: EventKind,
        text: impl Into<String>,
    ) {
        let actor = self.actor();
        self.emit_locus_except(locus, kind, text, &[actor]);
    }

    /// Third-person output to everyone in `locus` except the named entities. The
    /// general form of [`Ctx::emit_locus_except_self`]: a directed act (A waves at B)
    /// gives the actor and the target each their own line, then this to the locus so
    /// neither party reads the bystander view a second time.
    pub fn emit_locus_except(
        &mut self,
        locus: EntityId,
        kind: EventKind,
        text: impl Into<String>,
        exclude: &[EntityId],
    ) {
        self.out.push(Outbound::excluding(
            Event::to_locus(locus, kind, text),
            exclude.to_vec(),
        ));
    }
}

/// A tick-loop system: the simulation-side analogue of a verb [`Handler`]. It
/// mutates the world and emits semantic output through a [`SystemCtx`], which the
/// runtime resolves to connections the same way it does a verb's. An app registers
/// these in its `App.systems`; the engine only invokes them.
///
/// [`Handler`]: crate::Handler
pub type System = fn(&mut SystemCtx);

/// The per-tick context handed to a [`System`]. Mirrors [`Ctx`] for the
/// simulation half: the world a system mutates and the output buffer it emits
/// into, plus both clocks. There is no actor or connection, because a system acts
/// on the world's behalf, not a player's, so its output is third-person only.
///
/// Both clocks are carried even when a system uses only one: `tick` is
/// deterministic sim time (the default for app logic) and `now` is wall-clock
/// (for real-world scheduling). They come straight from the runtime's per-tick
/// context, captured once so every system in a tick sees the same instant.
///
/// `facts` is the tick's structural-fact batch: an observation stream of the
/// world mutations that have committed (destructions, and more as consumers need
/// them). A reaction system iterates it; a non-reactive system ignores it. The
/// slice borrows a buffer the runtime drained once before the system loop, so a
/// system never sees a fact another system in the same pass emitted.
pub struct SystemCtx<'a> {
    pub world: &'a mut World,
    pub tick: u64,
    pub now: SystemTime,
    pub facts: &'a [Fact],
    out: &'a mut Vec<Outbound>,
}

impl<'a> SystemCtx<'a> {
    pub fn new(
        world: &'a mut World,
        tick: u64,
        now: SystemTime,
        facts: &'a [Fact],
        out: &'a mut Vec<Outbound>,
    ) -> Self {
        SystemCtx {
            world,
            tick,
            now,
            facts,
            out,
        }
    }

    /// Third-person output to everyone in `locus`. A system has no first person, so
    /// unlike [`Ctx::emit_locus_except_self`] there is no actor to exclude.
    pub fn emit_locus(&mut self, locus: EntityId, kind: EventKind, text: impl Into<String>) {
        self.out
            .push(Outbound::new(Event::to_locus(locus, kind, text)));
    }

    /// The world and the raw output buffer together, the [`Ctx::world_and_out`]
    /// analogue for the simulation half: a tick system driving an autonomous agent
    /// through the shared narrating perform hands the routine these two borrows, so
    /// an NPC's act narrates to the room from the same code a player's does.
    pub fn world_and_out(&mut self) -> (&mut World, &mut Vec<Outbound>) {
        (self.world, self.out)
    }

    /// Run an autonomous canonical action through the same performer as a player.
    /// Systems have no account principal, so the app supplies the deliberate
    /// default verdict instead of deriving authority from the chosen actor.
    pub fn perform(
        &mut self,
        registry: &AffordanceRegistry,
        verdict: &Verdict,
        action: &GroundAction,
    ) -> Result<PerformOutcome, PerformError> {
        registry.perform(self.world, self.out, verdict, action)
    }
}

/// Run `systems` over `world` for one tick and audience-resolve their output.
///
/// Drains the tick's structural facts once up front, so every system sees the same
/// batch and a fact a system emits buffers for the next tick rather than being seen
/// within this pass (making system order cosmetic). Each system mutates the world
/// and emits into its own buffer, which is then resolved to connections through
/// `emit` against `actors`, exactly as [`dispatch_command`] resolves a verb's
/// output.
///
/// This is the single system-loop implementation: the runtime's per-tick step
/// (`Dispatch::run_systems`) and the `tick_work` benchmark both call it, so neither
/// can drift from the other.
///
/// [`dispatch_command`]: crate::dispatch_command
pub fn run_systems(
    world: &mut World,
    systems: &[System],
    actors: &Actors,
    tick: u64,
    now: SystemTime,
    emit: &mut impl FnMut(Outgoing),
) {
    let facts = world.take_facts();
    for system in systems {
        let mut out: Vec<Outbound> = Vec::new();
        {
            let mut sctx = SystemCtx::new(world, tick, now, &facts, &mut out);
            system(&mut sctx);
        }
        for ob in out {
            resolve(world, actors, ob, emit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CapRegistry;

    #[test]
    fn caller_keeps_body_connection_and_account_authority_together() {
        let mut caps = CapRegistry::new();
        let build = caps.register("build");
        let ban = caps.register("ban");
        let verdict = Verdict::new([build].into_iter().collect(), false);
        let caller = Caller::new(EntityId(7), ConnectionId(3), &verdict);

        assert_eq!(caller.actor(), EntityId(7));
        assert_eq!(caller.conn(), ConnectionId(3));

        let mut world = World::new();
        let mut out = Vec::new();
        let mut ctx = Ctx::new(&mut world, caller, &mut out);
        assert_eq!(ctx.actor(), EntityId(7));
        assert_eq!(ctx.conn(), ConnectionId(3));
        assert!(ctx.permits(build));
        assert!(!ctx.permits(ban));
        assert!(!ctx.is_su());
        assert!(std::ptr::eq(ctx.verdict(), &verdict));

        ctx.cold_read("book", EventKind::Narration);
        assert!(matches!(
            ctx.cold_ops(),
            [ColdOp::Read { conn, .. }] if *conn == ConnectionId(3)
        ));
    }
}
