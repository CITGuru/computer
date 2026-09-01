//! Boxes that outlived the server.
//!
//! A box is a container and this server is a process, and the container is the
//! one that survives. Without this, a restart forgets every box it started
//! while they carry on running and charging for it.
//!
//! Each box carries its own spec in a label, so what comes back is a box this
//! server can drive *and* fork, rather than a name it has to guess about. What
//! does not come back is the trace: it lived in memory, and a box adopted after
//! a restart starts a new one saying so.

use crate::registry::Registry;
use crate::spec;
use crate::trace::Traces;
use crate::wire::{Actor, Placement, Spec, TraceEvent};
use computer::{Computer, DockerMachine, Machine, SystemDocker};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// What a box says it is, written where the runtime keeps it rather than where
/// this process does.
pub const BOX_LABEL: &str = "computer.server.box";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoxLabel {
    pub digest: String,
    pub spec: Spec,
    pub placement: Placement,
    pub width: u32,
    pub height: u32,
    pub screens: u32,
}

impl BoxLabel {
    /// A label value, or `None` where it would not serialise — a box that
    /// cannot describe itself is still worth starting, it just will not come
    /// back after a restart.
    pub fn encode(&self) -> Option<String> {
        serde_json::to_string(self).ok()
    }

    pub fn decode(value: &str) -> Option<Self> {
        serde_json::from_str(value).ok()
    }
}

/// The runtimes to look in, from `COMPUTER_SERVER_RUNTIMES`.
///
/// A box placed on a runtime nobody asks about stays lost, so the list is
/// configurable rather than assumed.
pub fn runtimes() -> Vec<String> {
    listed(std::env::var("COMPUTER_SERVER_RUNTIMES").ok().as_deref())
}

fn listed(named: Option<&str>) -> Vec<String> {
    let found: Vec<String> = named
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|runtime| !runtime.is_empty())
        .map(str::to_string)
        .collect();

    if found.is_empty() {
        return vec!["docker".to_string()];
    }
    found
}

/// Take back every box these runtimes are still holding.
///
/// Never fails: a runtime that is not installed is not an error at startup, and
/// a box that will not come back must not stop the ones that will.
pub async fn adopt(registry: &Registry, traces: &Traces, runtimes: &[String]) -> usize {
    let mut taken = 0;

    for runtime in runtimes {
        let machine: Arc<dyn Machine> = Arc::new(DockerMachine::new(Arc::new(SystemDocker::new(
            runtime.clone(),
        ))));

        let found = match machine.labelled(BOX_LABEL).await {
            Ok(found) => found,
            Err(error) => {
                tracing::debug!(%runtime, %error, "no boxes to take back from this runtime");
                continue;
            }
        };

        for (name, value) in found {
            match adopt_one(registry, traces, &machine, runtime, &name, &value).await {
                Ok(()) => taken += 1,
                Err(error) => tracing::warn!(
                    box_ = %name,
                    %runtime,
                    %error,
                    "a box is running that this server could not take back; it will \
                     keep its memory until something else removes it"
                ),
            }
        }
    }

    taken
}

/// Not an `ApiError`: nothing here is answering a request, and the only reader
/// is the log.
async fn adopt_one(
    registry: &Registry,
    traces: &Traces,
    machine: &Arc<dyn Machine>,
    runtime: &str,
    name: &str,
    value: &str,
) -> Result<(), String> {
    let Some(label) = BoxLabel::decode(value) else {
        return Err("its label is not one this server wrote".to_string());
    };

    let profile = spec::profile_for(label.spec.desktop.server);
    let computer = Computer::attach_using(Arc::clone(machine), name, profile, None)
        .await
        .map_err(|error| error.to_string())?;

    let entry = registry
        .insert(
            name.to_string(),
            label.digest.clone(),
            label.screens,
            label.width,
            label.height,
            computer,
        )
        .await;

    let trace = traces.of(&entry.id);
    trace.record(
        Actor::System,
        TraceEvent::BoxCreated {
            spec_digest: label.digest,
            spec: Box::new(label.spec),
            placement: Box::new(label.placement),
            width: label.width,
            height: label.height,
            screens: label.screens,
        },
    );
    trace.record(
        Actor::System,
        TraceEvent::Adopted {
            runtime: runtime.to_string(),
        },
    );

    tracing::info!(box_ = %entry.id, %runtime, "took a box back");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label() -> BoxLabel {
        BoxLabel {
            digest: "abc".to_string(),
            spec: Spec::default(),
            placement: Placement::default(),
            width: 1280,
            height: 800,
            screens: 1,
        }
    }

    #[test]
    fn test_a_box_describes_itself_well_enough_to_come_back() {
        let encoded = label().encode().expect("a label");

        assert_eq!(BoxLabel::decode(&encoded), Some(label()));
    }

    #[test]
    fn test_a_label_this_server_did_not_write_is_ignored_rather_than_trusted() {
        assert!(BoxLabel::decode("someone else's label").is_none());
        assert!(BoxLabel::decode(r#"{"digest":"abc"}"#).is_none());
    }

    #[test]
    fn test_docker_is_looked_in_when_nothing_says_otherwise() {
        assert_eq!(listed(None), vec!["docker".to_string()]);
        assert_eq!(listed(Some("  ")), vec!["docker".to_string()]);
    }

    #[test]
    fn test_the_runtimes_to_look_in_can_be_named() {
        assert_eq!(
            listed(Some("podman, nerdctl ,")),
            vec!["podman".to_string(), "nerdctl".to_string()]
        );
    }
}
