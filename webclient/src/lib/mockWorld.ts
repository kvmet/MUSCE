// A tiny stand-in for the sim, so the client is fully exercisable before the
// WebSocket transport exists. It mirrors `musce_ref::offers`: the same focus-role
// convention, the same guards in the same order, the same three-way classify. The
// seed matches the `offers` test fixture (a chest, a held coin, a loose button, a
// takeable rock, a locked gate). Replace this whole file's consumer with a real
// `WsConn` and none of the UI changes.

import type { Entity, Frame, Offer, OfferStatus, Role } from "./protocol";

interface Aff {
  name: string;
  focus: Role; // which role the pointed-at entity fills
  requires: Role[]; // roles the guards reference (mirrors `required_roles`)
  // Guards in order; each returns a refusal reason when it FAILS, else null.
  guards: Array<(w: MockWorld, f: Frame) => string | null>;
}

export class MockWorld {
  entities = new Map<number, Entity>();
  actor = 0;
  room = 0;

  constructor() {
    const add = (e: Entity) => this.entities.set(e.id, e);
    // ids are arbitrary but stable within a session
    add({ id: 1, name: "a stone hall", kinds: ["locus"], contents: [2, 3, 4, 5, 6] });
    add({ id: 2, name: "a weathered adventurer", kinds: ["player"], contents: [7] });
    add({ id: 3, name: "a wooden chest", kinds: ["container"], contents: [] });
    add({ id: 4, name: "a smooth rock", kinds: ["item"], contents: [] });
    add({ id: 5, name: "a stray button", kinds: ["item"], contents: [] });
    add({ id: 6, name: "north", kinds: ["exit", "locked"], contents: [] });
    add({ id: 7, name: "a copper coin", kinds: ["item"], contents: [] });
    this.actor = 2;
    this.room = 1;
  }

  private has(id: number | undefined, kind: string): boolean {
    return id !== undefined && (this.entities.get(id)?.kinds.includes(kind) ?? false);
  }

  private parentOf(id: number): number | undefined {
    for (const e of this.entities.values()) if (e.contents.includes(id)) return e.id;
    return undefined;
  }

  private containedBy(a: number | undefined, b: number | undefined): boolean {
    return a !== undefined && b !== undefined && this.parentOf(a) === b;
  }

  private move(id: number, into: number) {
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

  // Mirrors `offers::classify`: completeness before veto, so an unbound role reads
  // as "pick something", not as the guard it would spuriously fail.
  classify(name: string, frame: Frame): OfferStatus {
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

  frameFor(name: string, clicked: number): Frame {
    const aff = this.table().find((a) => a.name === name)!;
    return { actor: this.actor, [aff.focus]: clicked };
  }

  offersOn(clicked: number): Offer[] {
    return this.table().map((a) => ({
      name: a.name,
      status: this.classify(a.name, this.frameFor(a.name, clicked)),
    }));
  }

  inventory(): Entity[] {
    const me = this.entities.get(this.actor)!;
    return me.contents.map((id) => this.entities.get(id)!);
  }

  name(id: number): string {
    return this.entities.get(id)?.name ?? `#${id}`;
  }

  // Apply a fully-bound act, refusing exactly as `perform` would. Returns the line
  // the player hears.
  perform(name: string, frame: Frame): string {
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
  send(line: string): string[] {
    const input = line.trim();
    if (!input) return [];
    const echo = `> ${input}`;
    if (input === "look" || input === "l") {
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
