// The client's view of a snapshot. The wire `SnapshotData` carries entities as an
// array (the order they were walked); the UI indexes them by id while rendering the
// tree, so the store holds a `Map` keyed by id instead. This is the one wire type
// the client reshapes: everything else in `bindings/` is consumed as generated.

import type { Entity } from "./bindings/Entity";
import type { SnapshotData } from "./bindings/SnapshotData";

export interface Snapshot {
  root: string; // the actor's room
  actor: string;
  entities: Map<string, Entity>;
}

export function toSnapshot(data: SnapshotData): Snapshot {
  const entities = new Map<string, Entity>();
  for (const e of data.entities) entities.set(e.id, e);
  return { root: data.root, actor: data.actor, entities };
}
