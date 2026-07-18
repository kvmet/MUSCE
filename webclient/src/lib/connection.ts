// The seam between the UI and the server. The UI programs against `Conn` only, so
// the mock and the eventual WebSocket transport are interchangeable. This mirrors
// the engine's own `Connection` abstraction: one interface, many backends.

import type { Entity, Frame, Offer, OfferStatus, Snapshot } from "./protocol";
import { MockWorld } from "./mockWorld";

export interface Conn {
  snapshot(): Snapshot;
  offersOn(entityId: number): Offer[];
  frameFor(name: string, clicked: number): Frame;
  classify(name: string, frame: Frame): OfferStatus;
  perform(name: string, frame: Frame): string;
  inventory(): Entity[];
  name(id: number): string;
  send(line: string): string[];
}

// Backed by the in-browser mock. `snapshot` returns a fresh object each call so a
// reassignment is a new reference the UI's reactivity can track.
export class MockConn implements Conn {
  private world = new MockWorld();

  snapshot(): Snapshot {
    // Fresh entity objects and arrays each call: the UI's reactivity is
    // identity-based, and the real server sends fresh data per read anyway, so a
    // shared mutable handle would (and did) make the tree miss in-place changes.
    const entities = new Map<number, Entity>();
    for (const [id, e] of this.world.entities) {
      entities.set(id, { ...e, kinds: [...e.kinds], contents: [...e.contents] });
    }
    return { root: this.world.room, actor: this.world.actor, entities };
  }
  offersOn(id: number): Offer[] {
    return this.world.offersOn(id);
  }
  frameFor(name: string, clicked: number): Frame {
    return this.world.frameFor(name, clicked);
  }
  classify(name: string, frame: Frame): OfferStatus {
    return this.world.classify(name, frame);
  }
  perform(name: string, frame: Frame): string {
    return this.world.perform(name, frame);
  }
  inventory(): Entity[] {
    return this.world.inventory();
  }
  name(id: number): string {
    return this.world.name(id);
  }
  send(line: string): string[] {
    return this.world.send(line);
  }
}
