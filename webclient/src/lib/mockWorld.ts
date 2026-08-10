// A small offline stand-in for the reference app's generic pointing surface. It
// exposes only the app-defined canonical `give` affordance and speaks the same
// generated protocol as the live host.

import type { AffordanceValue } from "./bindings/AffordanceValue";
import type { ClientMsg } from "./bindings/ClientMsg";
import type { Entity } from "./bindings/Entity";
import type { Offer } from "./bindings/Offer";
import type { ParameterBinding } from "./bindings/ParameterBinding";
import type { ServerMsg } from "./bindings/ServerMsg";
import type { SnapshotData } from "./bindings/SnapshotData";

const parameters = [
  { id: "item", label: "item", sort: { kind: "entity" } as const, mode: "input" as const },
  {
    id: "recipient",
    label: "recipient",
    sort: { kind: "entity" } as const,
    mode: "input" as const,
  },
];

export class MockWorld {
  entities = new Map<string, Entity>();
  actor = "2";
  room = "1";

  constructor() {
    const add = (entity: Entity) => this.entities.set(entity.id, entity);
    add(node("1", "a stone hall", ["locus"], ["2", "3", "4"]));
    add(node("2", "a weathered adventurer", ["player"], ["5"]));
    add(node("3", "a brass drone", ["creature", "giftRecipient"], []));
    add(node("4", "a wooden chest", ["container"], []));
    add(node("5", "a copper coin", ["item"], []));
  }

  handle(msg: ClientMsg): ServerMsg[] {
    switch (msg.t) {
      case "line":
        return this.line(msg.line).map((text) => ({ t: "event", kind: "feedback", text }));
      case "query":
        return msg.q === "snapshot"
          ? [{ t: "snapshot", ...this.snapshot() }]
          : [{ t: "offers", clicked: msg.clicked, offers: this.offers(msg.clicked) }];
      case "perform": {
        const result = this.perform(msg.affordance, msg.inputs);
        if (typeof result === "string") {
          return [{ t: "event", kind: "feedback", text: result }];
        }
        return [
          { t: "event", kind: "feedback", text: result.feedback },
          { t: "performed", affordance: msg.affordance, results: [] },
        ];
      }
    }
  }

  offers(clicked: string): Offer[] {
    if (!this.entities.has(clicked)) return [];
    const recipient: ParameterBinding = {
      parameter: "recipient",
      value: { kind: "entity", id: clicked },
    };
    const values = (this.entities.get(this.actor)?.contents ?? []).map(
      (id): AffordanceValue => ({ kind: "entity", id }),
    );
    return [
      {
        affordance: "give",
        display_name: "Give",
        parameters,
        bindings: [recipient],
        candidates: [{ parameter: "item", values }],
        status: this.has(clicked, "giftRecipient")
          ? { kind: "needs", parameters: ["item"] }
          : { kind: "vetoed", reason: "You can't give things to that." },
      },
    ];
  }

  snapshot(): SnapshotData {
    return { root: this.room, actor: this.actor, entities: [...this.entities.values()] };
  }

  private perform(affordance: string, bindings: ParameterBinding[]): { feedback: string } | string {
    if (affordance !== "give") return "You can't do that.";
    const item = entityBinding(bindings, "item");
    const recipient = entityBinding(bindings, "recipient");
    if (!item || !recipient) return "That action is malformed.";
    if (!this.containedBy(item, this.actor)) return "You aren't carrying that.";
    if (!this.has(recipient, "giftRecipient")) return "You can't give things to that.";
    this.move(item, recipient);
    return { feedback: `You give ${this.name(item)} to ${this.name(recipient)}.` };
  }

  private has(id: string, kind: string): boolean {
    return this.entities.get(id)?.kinds.includes(kind) ?? false;
  }

  private parentOf(id: string): string | undefined {
    for (const entity of this.entities.values()) {
      if (entity.contents.includes(id)) return entity.id;
    }
    return undefined;
  }

  private containedBy(entity: string, container: string): boolean {
    return this.parentOf(entity) === container;
  }

  private move(id: string, into: string) {
    const from = this.parentOf(id);
    if (from !== undefined) {
      const container = this.entities.get(from)!;
      container.contents = container.contents.filter((child) => child !== id);
    }
    this.entities.get(into)!.contents.push(id);
  }

  private name(id: string): string {
    return this.entities.get(id)?.name ?? `#${id}`;
  }

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

function entityBinding(bindings: ParameterBinding[], parameter: string): string | undefined {
  const value = bindings.find((binding) => binding.parameter === parameter)?.value;
  return value?.kind === "entity" ? value.id : undefined;
}

function node(id: string, name: string, kinds: string[], contents: string[]): Entity {
  return { id, name, kinds, contents, details: [] };
}
