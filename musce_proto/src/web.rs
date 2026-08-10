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

// Entity ids cross this wire as strings, not JSON numbers. A `musce_core::EntityId`
// is a u64, and a JSON number is an IEEE double, so an id using its high bits would
// lose precision in a browser's `JSON.parse`. A string is lossless regardless of how
// ids are allocated (the counter today, sharded or hashed ids later), and the field
// can hold a richer id form (a URI) with no wire-type change. The app formats an id
// into the string on the way out and parses it back on the way in.

/// A message from the web client. The transport parses it and hands the sim a
/// typed [`Input`]. Lifecycle (`Connected`/`Disconnected`) is transport-generated,
/// never sent by a client, so it is absent here.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum ClientMsg {
    /// A text command, exactly what a telnet client would type.
    Line { line: String },
    /// A read query: pure, no world mutation, no narration.
    Query(Query),
    /// Perform an app-defined affordance with typed, named input bindings.
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

/// A complete canonical act request. Bindings name app-defined input parameters;
/// there are no engine-defined roles or fixed arity. Actor identity comes from the
/// authenticated session and results are server-produced only.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Perform {
    pub affordance: String,
    pub inputs: Vec<ParameterBinding>,
}

/// A read the client asks of the world, answered by a [`ServerMsg`] reply; it never
/// mutates. Performing an offered action is a command, not a query.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "q", rename_all = "lowercase")]
pub enum Query {
    /// The perceivable containment tree for the actor (its room and, nested, what
    /// it holds).
    Snapshot,
    /// The affordances available on the clicked entity ("what can I do to this?").
    Offers { clicked: String },
}

/// A message to the web client: a streamed event or a query reply, tagged so the
/// client can route it. The transport serializes this; the sim produces the
/// in-process forms (`Delivery` for events, this enum for replies).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum ServerMsg {
    /// Streamed narration/feedback, the same content the text pipe renders.
    Event { kind: EventKind, text: String },
    /// The reply to [`Query::Snapshot`].
    Snapshot(SnapshotData),
    /// The reply to [`Query::Offers`].
    Offers { clicked: String, offers: Vec<Offer> },
    /// Typed results from a successfully committed [`Perform`] request.
    Performed(Performed),
}

/// One node of the containment tree: a projection of the world's containment
/// relation plus the app's name and kinds for the entity, never new state.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Entity {
    pub id: String,
    pub name: String,
    /// App kind tags (e.g. "container", "item", "exit", "locked").
    pub kinds: Vec<String>,
    /// Ids directly contained by this entity.
    pub contents: Vec<String>,
    /// App-projected passive detail as ordered `(label, value)` pairs (e.g. a
    /// `("description", ...)`). Opaque to the wire: the app decides what an actor
    /// perceives by presence and the client just paints the pairs. This is the
    /// same prose a narrated `examine` reveals, delivered silently as part of the
    /// read, so the pointing client renders a focused entity without a round-trip.
    pub details: Vec<(String, String)>,
}

/// The snapshot reply payload: the tree root (the actor's room), the actor, and
/// every perceivable entity. `entities[actor].contents` is the actor's inventory,
/// so it needs no separate query.
#[derive(Debug, Clone, Default, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SnapshotData {
    pub root: String,
    pub actor: String,
    pub entities: Vec<Entity>,
}

/// One app-exposed partial canonical grounding and its engine classification.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Offer {
    pub affordance: String,
    pub display_name: String,
    pub parameters: Vec<ParameterDecl>,
    pub bindings: Vec<ParameterBinding>,
    pub candidates: Vec<InputCandidates>,
    pub status: OfferStatus,
}

/// Current classification of an app-exposed partial grounding.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum OfferStatus {
    Available,
    Vetoed { reason: String },
    Needs { parameters: Vec<String> },
}

/// Canonical parameter mode. The client may bind inputs only; results are shown
/// in signatures and returned by the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum ParameterMode {
    Input,
    Result,
}

/// Canonical value sort in a wire-safe representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ParameterSort {
    Entity,
    Text,
    Symbol { domain: String },
}

/// One app-defined parameter declaration carried in an offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ParameterDecl {
    pub id: String,
    pub label: String,
    pub sort: ParameterSort,
    pub mode: ParameterMode,
}

/// One typed canonical value. Entity ids remain strings to preserve all u64 bits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AffordanceValue {
    Entity { id: String },
    Text { text: String },
    Symbol { domain: String, value: String },
}

/// A value bound to one stable app-defined parameter id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ParameterBinding {
    pub parameter: String,
    pub value: AffordanceValue,
}

/// App-selected presentation candidates for one missing input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct InputCandidates {
    pub parameter: String,
    pub values: Vec<AffordanceValue>,
}

/// Successful typed results from one committed affordance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Performed {
    pub affordance: String,
    pub results: Vec<ParameterBinding>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The serde-compat translation of the risky enums is pinned to the shape the
    /// hand-written client bound to. ts-rs renders an internally-tagged newtype
    /// variant as a `{tag} & Inner` intersection, which is structurally the wire
    /// object (`{"t":"query","q":"snapshot"}`); this asserts that form survives a
    /// ts-rs bump or a serde-attr change, since a silent shift there would mistype
    /// the client with no compile error on the Rust side. `decl()` returns the
    /// declaration without writing to disk, so this needs no export dir.
    #[cfg(feature = "ts")]
    #[test]
    fn generated_enum_shapes_are_pinned() {
        use ts_rs::{Config, TS};

        let cfg = Config::default();

        let client = ClientMsg::decl(&cfg);
        assert!(client.contains(r#""t": "line""#), "{client}");
        assert!(client.contains(r#""t": "query" } & Query"#), "{client}");
        assert!(client.contains(r#""t": "perform" } & Perform"#), "{client}");

        let server = ServerMsg::decl(&cfg);
        assert!(server.contains(r#""t": "event""#), "{server}");
        assert!(
            server.contains(r#""t": "snapshot" } & SnapshotData"#),
            "{server}"
        );
        assert!(server.contains(r#""t": "offers""#), "{server}");
        assert!(server.contains(r#""t": "performed""#), "{server}");

        let status = OfferStatus::decl(&cfg);
        assert!(status.contains(r#""kind": "available""#), "{status}");
        assert!(status.contains(r#""kind": "vetoed""#), "{status}");
        assert!(status.contains(r#""kind": "needs""#), "{status}");

        // Entity ids cross as strings (see the module note), so ts-rs renders them
        // `string`, not the `bigint` its default u64 mapping would emit.
        let perform = Perform::decl(&cfg);
        assert!(perform.contains("affordance: string"), "{perform}");
        assert!(
            perform.contains("inputs: Array<ParameterBinding>"),
            "{perform}"
        );
        let snapshot = SnapshotData::decl(&cfg);
        assert!(snapshot.contains("root: string"), "{snapshot}");
    }

    /// The nested-tag envelope round-trips: `{"t":"query","q":"offers",...}` parses
    /// through the outer `t` tag and the inner `q` tag both. This is the exact shape
    /// a front-end sends, and the serde nesting is the riskiest part, so pin it.
    #[test]
    fn query_envelope_parses_through_both_tags() {
        let snap: ClientMsg = serde_json::from_str(r#"{"t":"query","q":"snapshot"}"#).unwrap();
        assert!(matches!(snap.into_input(), Input::Query(Query::Snapshot)));

        let offers: ClientMsg =
            serde_json::from_str(r#"{"t":"query","q":"offers","clicked":"42"}"#).unwrap();
        assert!(matches!(
            offers.into_input(),
            Input::Query(Query::Offers { clicked }) if clicked == "42"
        ));

        let line: ClientMsg = serde_json::from_str(r#"{"t":"line","line":"look"}"#).unwrap();
        assert!(matches!(line.into_input(), Input::Line(l) if l == "look"));
    }

    /// A perform request carries stable parameter ids and typed values, with no
    /// engine-defined role or fixed arity.
    #[test]
    fn perform_parses_generic_typed_bindings() {
        let give: ClientMsg = serde_json::from_str(
            r#"{"t":"perform","affordance":"give","inputs":[{"parameter":"item","value":{"kind":"entity","id":"5"}},{"parameter":"recipient","value":{"kind":"entity","id":"9"}}]}"#,
        )
        .expect("give grounding parses");
        assert!(matches!(
            give.into_input(),
            Input::Perform(Perform { affordance, inputs })
                if affordance == "give" && inputs.len() == 2
        ));
    }

    /// The reply envelope inlines its payload under the `t` tag, and the status
    /// discriminator is the `kind` / `camelCase` shape the client renders on.
    #[test]
    fn server_messages_serialize_to_the_wire_shape() {
        let snap = ServerMsg::Snapshot(SnapshotData {
            root: "1".into(),
            actor: "7".into(),
            entities: vec![Entity {
                id: "7".into(),
                name: "you".into(),
                kinds: vec!["player".into()],
                contents: vec![],
                details: vec![("description".into(), "a weary adventurer".into())],
            }],
        });
        let json = serde_json::to_string(&snap).unwrap();
        assert!(json.contains(r#""t":"snapshot""#));
        assert!(json.contains(r#""root":"1""#));
        // The detail bag rides the wire as ordered [label, value] pairs.
        assert!(json.contains(r#""details":[["description","a weary adventurer"]]"#));

        let offers = ServerMsg::Offers {
            clicked: "42".into(),
            offers: vec![Offer {
                affordance: "give".into(),
                display_name: "Give".into(),
                parameters: vec![ParameterDecl {
                    id: "item".into(),
                    label: "item".into(),
                    sort: ParameterSort::Entity,
                    mode: ParameterMode::Input,
                }],
                bindings: Vec::new(),
                candidates: Vec::new(),
                status: OfferStatus::Needs {
                    parameters: vec!["item".into()],
                },
            }],
        };
        let json = serde_json::to_string(&offers).unwrap();
        assert!(json.contains(r#""t":"offers""#));
        assert!(json.contains(r#""kind":"needs""#));
        assert!(json.contains(r#""parameters":["item"]"#));

        let performed = ServerMsg::Performed(Performed {
            affordance: "label".into(),
            results: vec![ParameterBinding {
                parameter: "old_label".into(),
                value: AffordanceValue::Text {
                    text: "crate".into(),
                },
            }],
        });
        let json = serde_json::to_string(&performed).unwrap();
        assert!(json.contains(r#""t":"performed""#));
        assert!(json.contains(r#""parameter":"old_label""#));
        assert!(json.contains(r#""kind":"text","text":"crate""#));
    }
}
