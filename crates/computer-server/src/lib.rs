//! A REST API over `computer` boxes.
//!
//! Two halves. Lifecycle — create, list, remove — is ordinary REST. Driving
//! is the batch at `POST /v1/boxes/{id}/screens/{n}/actions`, which takes a
//! run of actions and hands back the frame they produced, because an agent's
//! step is several actions and one look, and one request per click spends a
//! round trip on each.

pub mod error;
pub mod extract;
pub mod idempotency;
pub mod recover;
pub mod registry;
pub mod routes;
pub mod spec;
pub mod trace;
pub mod wire;

use idempotency::Replies;
use registry::Registry;
use trace::Traces;

#[derive(Default)]
pub struct AppState {
    pub registry: Registry,
    pub replies: Replies,
    /// Kept beside the registry rather than on a box, because removing a box
    /// must not remove the record of what was done in it.
    pub traces: Traces,
}
