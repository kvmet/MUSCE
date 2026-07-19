// A tiny stand-in for the sim, so the client is fully exercisable offline. It mirrors
// `musce_ref`: the same focus-role convention, the same guards in the same order, the
// same three-way classify, and it answers the same wire messages the real server
// does (`handle` maps a `ClientMsg` to the `ServerMsg`s it provokes). The seed matches
// the `offers` fixture (a chest, a held coin, a loose button, a takeable rock, a
// locked gate). Ids are strings, as they cross the real wire.

import type { ClientMsg } from "./bindings/ClientMsg";
import type { ServerMsg } from "./bindings/ServerMsg";
import type { Entity } from "./bindings/Entity";
import type { Offer } from "./bindings/Offer";
import type { OfferStatus } from "./bindings/OfferStatus";
import type { Role } from "./bindings/Role";
import type { SnapshotData } from "./bindings/SnapshotData";

// The case frame the mock resolves an act against: which entities fill its roles.
// Client-internal; the wire carries `focus`/`with`, and the mock maps them onto this
// exactly as the server's game policy does.
interface Frame {
  actor: string;
  object?: string;
  target?: string;
}

interface Aff {
  name: string;
  focus: Role; // which role the pointed-at entity fills
  requires: Role[]; // roles the guards reference (mirrors `required_roles`)
  // Guards in order; each returns a refusal reason when it FAILS, else null.
  guards: Array<(w: MockWorld, f: Frame) => string | null>;
}

export class MockWorld {
  entities = new Map<string, Entity>();
  actor = "2";
  room = "1";

  constructor() {
    const add = (e: Entity) => this.entities.set(e.id, e);
    // ids are arbitrary but stable within a session
    add(node("1", "a stone hall", ["locus"], ["2", "3", "4", "5", "6"]));
    add(node("2", "a weathered adventurer", ["player"], ["7"]));
    add(node("3", "a wooden chest", ["container"], []));
    add(node("4", "a smooth rock", ["item"], []));
    add(node("5", "a stray button", ["item"], []));
    add(node("6", "north", ["exit", "locked"], []));
    add(node("7", "a copper coin", ["item"], []));
  }

  // Map a wire message to the replies it provokes, exactly as the sim would.
  handle(msg: ClientMsg): ServerMsg[] {
    switch (msg.t) {
      case "line":
        return this.line(msg.line).map((text) => ({ t: "event", kind: "feedback", text }));
      case "query":
        return msg.q === "snapshot"
          ? [{ t: "snapshot", ...this.snapshot() }]
          : [{ t: "offers", clicked: msg.clicked, offers: this.offers(msg.clicked) }];
      case "perform":
        return [
          {
            t: "event",
            kind: "feedback",
            text: this.perform(msg.name, msg.focus, msg.with ?? undefined),
          },
        ];
    }
  }

  private has(id: string | undefined, kind: string): boolean {
    return id !== undefined && (this.entities.get(id)?.kinds.includes(kind) ?? false);
  }

  private parentOf(id: string): string | undefined {
    for (const e of this.entities.values()) if (e.contents.includes(id)) return e.id;
    return undefined;
  }

  private containedBy(a: string | undefined, b: string | undefined): boolean {
    return a !== undefined && b !== undefined && this.parentOf(a) === b;
  }

  private move(id: string, into: string) {
    const from = this.parentOf(id);
    if (from !== undefined) {
      const c = this.entities.get(from)!;
      c.contents = c.contents.filter((x) => x !== id);
    }
    this.entities.get(into)!.contents.push(id);
  }

  private table(): Aff[] {
    return [
      {
        name: "take",
        focus: "object",
        requires: ["object"],
        // takeable: not a locus, player, or creature
        guards: [
          (w, f) =>
            w.has(f.object, "locus") || w.has(f.object, "player") || w.has(f.object, "creature")
              ? "You can't take that."
              : null,
        ],
      },
      {
        name: "drop",
        focus: "object",
        requires: ["object"],
        guards: [(w, f) => (w.containedBy(f.object, w.actor) ? null : "You aren't carrying that.")],
      },
      {
        name: "put",
        focus: "target",
        requires: ["object", "target"],
        guards: [
          (w, f) => (w.containedBy(f.object, w.actor) ? null : "You aren't carrying that."),
          (w, f) => (w.has(f.target, "container") ? null : "You can't put things in that."),
        ],
      },
      {
        name: "eat",
        focus: "object",
        requires: ["object"],
        guards: [
          (w, f) =>
            w.containedBy(f.object, w.actor) && w.has(f.object, "edible")
              ? null
              : "You have nothing edible to eat.",
        ],
      },
      {
        name: "go",
        focus: "target",
        requires: ["target"],
        guards: [(w, f) => (w.has(f.target, "locked") ? "It's locked." : null)],
      },
    ];
  }

  private filled(frame: Frame, role: Role): boolean {
    return frame[role] !== undefined;
  }

  // Mirrors `offers::classify`: completeness before veto, so an unbound role reads as
  // "pick something", not as the guard it would spuriously fail.
  private classify(name: string, frame: Frame): OfferStatus {
    const aff = this.table().find((a) => a.name === name)!;
    for (const role of aff.requires) {
      if (!this.filled(frame, role)) return { kind: "needsRole", role };
    }
    for (const g of aff.guards) {
      const reason = g(this, frame);
      if (reason) return { kind: "vetoed", reason };
    }
    return { kind: "available" };
  }

  private frameForRole(aff: Aff, clicked: string): Frame {
    return { actor: this.actor, [aff.focus]: clicked };
  }

  offers(clicked: string): Offer[] {
    return this.table().map((a) => ({
      name: a.name,
      status: this.classify(a.name, this.frameForRole(a, clicked)),
    }));
  }

  private name(id: string): string {
    return this.entities.get(id)?.name ?? `#${id}`;
  }

  snapshot(): SnapshotData {
    return { root: this.room, actor: this.actor, entities: [...this.entities.values()] };
  }

  // Apply a fully-grounded act named by the client: `focus` fills the affordance's
  // focus role, an optional `with` fills the other required role (the `put` object
  // once the container is the focus). Refuses exactly as the server's perform would;
  // returns the feedback line the actor hears.
  perform(name: string, focus: string, withId?: string): string {
    const aff = this.table().find((a) => a.name === name);
    if (!aff) return "You can't do that.";
    const frame: Frame = { actor: this.actor, [aff.focus]: focus };
    if (withId !== undefined) {
      const other = aff.requires.find((r) => r !== aff.focus);
      if (other) frame[other] = withId;
    }

    const status = this.classify(name, frame);
    if (status.kind === "vetoed") return status.reason;
    if (status.kind === "needsRole") return "You need to choose what to act on.";
    switch (name) {
      case "take":
        this.move(frame.object!, this.actor);
        return `You take ${this.name(frame.object!)}.`;
      case "drop":
        this.move(frame.object!, this.room);
        return `You drop ${this.name(frame.object!)}.`;
      case "put":
        this.move(frame.object!, frame.target!);
        return `You put ${this.name(frame.object!)} in ${this.name(frame.target!)}.`;
      case "eat":
        this.entities.delete(frame.object!);
        return `You eat ${this.name(frame.object!)}. Sated.`;
      case "go":
        return `You head ${this.name(frame.target!)}.`;
      default:
        return "You can't do that.";
    }
  }

  // A minimal text path, so the command bar coexists with clicking (the a11y point).
  private line(input: string): string[] {
    const text = input.trim();
    if (!text) return [];
    if (text === "@play") return [`Welcome. You are ${this.name(this.actor)}.`];
    const echo = `> ${text}`;
    if (text === "look" || text === "l") {
      const room = this.entities.get(this.room)!;
      const here = room.contents
        .filter((id) => id !== this.actor)
        .map((id) => this.name(id))
        .join(", ");
      return [echo, room.name, here ? `You see: ${here}.` : "It is empty."];
    }
    return [echo, "(the graybox only understands 'look' over text for now)"];
  }
}

// One seed node as a wire `Entity`. The mock exposes no passive detail, so `details`
// is empty; the field exists because the wire type carries it.
function node(id: string, name: string, kinds: string[], contents: string[]): Entity {
  return { id, name, kinds, contents, details: [] };
}
