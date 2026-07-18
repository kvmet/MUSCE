//! The structured JSON envelope the pointing web client speaks over the WebSocket
//! transport, alongside the plain text line pipe. Client messages ([`ClientMsg`])
//! carry text commands and read queries; server messages ([`ServerMsg`]) carry
//! streamed events and query replies.
//!
//! Unlike the in-process `Command`/`Outgoing` vocabulary in the crate root, these
//! types derive serde: they cross the wire as JSON, so their shapes are a contract
//! a front-end binds to (and are generated to TypeScript with `ts-rs`). The
//! transport (`musce_net::ws`) parses a `ClientMsg` into an [`Input`] and
//! serializes a `ServerMsg`, so the sim only ever sees typed values; a telnet
//! client, which speaks bare text, never produces or consumes these.
//!
//! See `docs/architecture/networking-and-sessions.md`.

use serde::{Deserialize, Serialize};

use crate::{EventKind, Input};

/// A message from the web client. The transport parses it and hands the sim a
/// typed [`Input`]. Lifecycle (`Connected`/`Disconnected`) is transport-generated,
/// never sent by a client, so it is absent here.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum ClientMsg {
    /// A text command, exactly what a telnet client would type.
    Line { line: String },
    /// A read query: pure, no world mutation, no narration.
    Query(Query),
    /// Act on a clicked entity by id, skipping name resolution: the affordance
    /// the client picked, the entity it clicked, and any second entity a
    /// role sub-pick supplied.
    Perform(Perform),
}

impl ClientMsg {
    /// Map the wire message into the sim's input vocabulary.
    pub fn into_input(self) -> Input {
        match self {
            ClientMsg::Line { line } => Input::Line(line),
            ClientMsg::Query(query) => Input::Query(query),
            ClientMsg::Perform(perform) => Input::Perform(perform),
        }
    }
}

/// A grounded act request: the client already holds the entity id, so this carries
/// the affordance name and its bound entities directly, with no noun to resolve.
/// `focus` is the clicked entity; `with` is the optional second entity a role
/// sub-pick supplied (the object to `put`, once the container is the focus). Which
/// role `focus` fills is game policy, so the game maps `focus`/`with` onto the
/// affordance's roles.
#[derive(Debug, Clone, Deserialize)]
pub struct Perform {
    pub name: String,
    pub focus: u64,
    #[serde(default)]
    pub with: Option<u64>,
}

/// A read the client asks of the world, answered by a [`ServerMsg`] reply; it never
/// mutates. Acting on an entity (`perform`) is a command, not a query, and lands in
/// a later slice.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "q", rename_all = "lowercase")]
pub enum Query {
    /// The perceivable containment tree for the actor (its room and, nested, what
    /// it holds).
    Snapshot,
    /// The affordances available on the clicked entity ("what can I do to this?").
    Offers { clicked: u64 },
}

/// A message to the web client: a streamed event or a query reply, tagged so the
/// client can route it. The transport serializes this; the sim produces the
/// in-process forms (`Delivery` for events, this enum for replies).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum ServerMsg {
    /// Streamed narration/feedback, the same content the text pipe renders.
    Event { kind: EventKind, text: String },
    /// The reply to [`Query::Snapshot`].
    Snapshot(SnapshotData),
    /// The reply to [`Query::Offers`].
    Offers { clicked: u64, offers: Vec<Offer> },
}

/// One node of the containment tree: a projection of the world's containment
/// relation plus the game's name and kinds for the entity, never new state.
#[derive(Debug, Clone, Serialize)]
pub struct Entity {
    pub id: u64,
    pub name: String,
    /// Game kind tags (e.g. "container", "item", "exit", "locked").
    pub kinds: Vec<String>,
    /// Ids directly contained by this entity.
    pub contents: Vec<u64>,
    /// Game-projected passive detail as ordered `(label, value)` pairs (e.g. a
    /// `("description", ...)`). Opaque to the wire: the game decides what an actor
    /// perceives by presence and the client just paints the pairs. This is the
    /// same prose a narrated `examine` reveals, delivered silently as part of the
    /// read, so the pointing client renders a focused entity without a round-trip.
    pub details: Vec<(String, String)>,
}

/// The snapshot reply payload: the tree root (the actor's room), the actor, and
/// every perceivable entity. `entities[actor].contents` is the actor's inventory,
/// so it needs no separate query.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SnapshotData {
    pub root: u64,
    pub actor: u64,
    pub entities: Vec<Entity>,
}

/// One enumerated act on a clicked entity: the affordance name and its status. The
/// wire form of `musce_ref::offers::Offer`.
#[derive(Debug, Clone, Serialize)]
pub struct Offer {
    pub name: String,
    pub status: OfferStatus,
}

/// How an affordance stands for the clicked entity: a live control, a greyed one
/// carrying the reason, or one that opens a sub-pick for a still-unbound role.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum OfferStatus {
    Available,
    Vetoed { reason: String },
    NeedsRole { role: Role },
}

/// The frame role a still-unbound pick fills.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Object,
    Target,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The nested-tag envelope round-trips: `{"t":"query","q":"offers",...}` parses
    /// through the outer `t` tag and the inner `q` tag both. This is the exact shape
    /// a front-end sends, and the serde nesting is the riskiest part, so pin it.
    #[test]
    fn query_envelope_parses_through_both_tags() {
        let snap: ClientMsg = serde_json::from_str(r#"{"t":"query","q":"snapshot"}"#).unwrap();
        assert!(matches!(snap.into_input(), Input::Query(Query::Snapshot)));

        let offers: ClientMsg =
            serde_json::from_str(r#"{"t":"query","q":"offers","clicked":42}"#).unwrap();
        assert!(matches!(
            offers.into_input(),
            Input::Query(Query::Offers { clicked: 42 })
        ));

        let line: ClientMsg = serde_json::from_str(r#"{"t":"line","line":"look"}"#).unwrap();
        assert!(matches!(line.into_input(), Input::Line(l) if l == "look"));
    }

    /// A perform frame flattens its fields under the `t` tag, and `with` is optional
    /// (a single-role act omits it, a sub-pick supplies it).
    #[test]
    fn perform_frame_parses_with_an_optional_second_role() {
        let take: ClientMsg = serde_json::from_str(r#"{"t":"perform","name":"take","focus":5}"#)
            .expect("take frame parses");
        assert!(matches!(
            take.into_input(),
            Input::Perform(Perform { name, focus: 5, with: None }) if name == "take"
        ));

        let put: ClientMsg =
            serde_json::from_str(r#"{"t":"perform","name":"put","focus":9,"with":5}"#)
                .expect("put frame parses");
        assert!(matches!(
            put.into_input(),
            Input::Perform(Perform {
                name,
                focus: 9,
                with: Some(5)
            }) if name == "put"
        ));
    }

    /// The reply envelope inlines its payload under the `t` tag, and the status
    /// discriminator is the `kind` / `camelCase` shape the client renders on.
    #[test]
    fn server_messages_serialize_to_the_wire_shape() {
        let snap = ServerMsg::Snapshot(SnapshotData {
            root: 1,
            actor: 7,
            entities: vec![Entity {
                id: 7,
                name: "you".into(),
                kinds: vec!["player".into()],
                contents: vec![],
                details: vec![("description".into(), "a weary adventurer".into())],
            }],
        });
        let json = serde_json::to_string(&snap).unwrap();
        assert!(json.contains(r#""t":"snapshot""#));
        assert!(json.contains(r#""root":1"#));
        // The detail bag rides the wire as ordered [label, value] pairs.
        assert!(json.contains(r#""details":[["description","a weary adventurer"]]"#));

        let offers = ServerMsg::Offers {
            clicked: 42,
            offers: vec![Offer {
                name: "put".into(),
                status: OfferStatus::NeedsRole { role: Role::Object },
            }],
        };
        let json = serde_json::to_string(&offers).unwrap();
        assert!(json.contains(r#""t":"offers""#));
        assert!(json.contains(r#""kind":"needsRole""#));
        assert!(json.contains(r#""role":"object""#));
    }
}
