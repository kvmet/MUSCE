// The wire vocabulary the client speaks, mirroring the Rust side. Today these are
// hand-written to match `musce_ref::offers` and the containment reads; once the
// WebSocket transport lands they should be generated from `musce_proto` with
// `ts-rs` so the shapes cannot drift.

export type Role = "object" | "target";

// The three-way status `musce_ref::offers::OfferStatus` produces. A pointing
// client renders each differently: a live control, a greyed one with the reason,
// or a control that opens a sub-pick for the still-unbound role.
export type OfferStatus =
  | { kind: "available" }
  | { kind: "vetoed"; reason: string }
  | { kind: "needsRole"; role: Role };

export interface Offer {
  name: string;
  status: OfferStatus;
}

// A case frame: which entities fill an affordance's roles. Mirrors
// `musce_action::Frame` (the `kind` preposition slot is unused by this graybox).
export interface Frame {
  actor: number;
  object?: number;
  target?: number;
}

// One node of the containment tree the client renders. This is the projection of
// `World::contents` / `container_of`, not new state.
export interface Entity {
  id: number;
  name: string;
  kinds: string[]; // "locus" | "player" | "creature" | "container" | "item" | "exit" | "edible" | "locked"
  contents: number[]; // ids directly contained
}

export interface Snapshot {
  root: number; // the actor's room
  actor: number;
  entities: Map<number, Entity>;
}
