use computer_api::TraceEvent;
use serde_json::{Value, json};

pub const SETTLE_MS: u64 = 1_500;
pub const PAGE: usize = 200;
pub const MOST: usize = 1_000;

pub fn of(event: &TraceEvent) -> Vec<(&'static str, Value)> {
    match event {
        TraceEvent::BoxCreated { spec_digest, .. } => vec![
            ("box.created", json!({ "spec_digest": spec_digest })),
            ("box.ready", json!({})),
        ],
        TraceEvent::BoxStarted | TraceEvent::BoxResumed => vec![("box.ready", json!({}))],
        TraceEvent::BoxPaused => vec![("box.paused", json!({}))],
        TraceEvent::BoxStopped => vec![("box.stopped", json!({}))],
        TraceEvent::BoxDeleted => vec![("box.removed", json!({}))],
        TraceEvent::Gone { why } if why == crate::reap::EXPIRED => {
            vec![("box.removed", json!({ "why": why }))]
        }
        TraceEvent::Gone { why } => vec![("box.unreachable", json!({ "why": why }))],
        TraceEvent::TakeoverStarted { screen, exclusive } => vec![(
            "screen.taken_over",
            json!({ "screen": screen, "exclusive": exclusive }),
        )],
        TraceEvent::TakeoverEnded { screen } => {
            vec![("screen.given_back", json!({ "screen": screen }))]
        }
        _ => Vec::new(),
    }
}
