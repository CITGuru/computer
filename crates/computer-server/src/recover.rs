//! Boxes that outlived the server.
//!
//! A box outlives this process, so without this a restart forgets every box it
//! started while they carry on running and charging for it. Each box carries its
//! own spec in a label, so what comes back is a box this server can drive *and*
//! fork rather than a name it has to guess about.
//!
//! Two kinds of place hold boxes and both are looked in. A container runtime is
//! on this host. A sandbox vendor is not, and a box there is the one that costs
//! money while nobody is watching it.

use crate::AppState;
use crate::spec;
use computer::sandboxes::remote::{self, RemoteApi};
use computer::{Computer, DockerMachine, Machine, Profile, SystemDocker};
use computer_api::{Actor, DisplayServer, Placement, Spec, TraceEvent};
use computer_storage::BoxRecord;
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

/// A box placed on a runtime nobody asks about stays lost, so the list is
/// configurable rather than assumed.
pub fn runtimes() -> Vec<String> {
    listed(std::env::var("COMPUTER_SERVER_RUNTIMES").ok().as_deref())
}

/// The sandbox vendors to ask, which is nothing unless a server says so.
///
/// No default, unlike runtimes: asking a vendor costs a call to somebody else's
/// control plane with somebody's credential, and a server that hands out
/// containers has neither.
pub fn sandboxes() -> Vec<String> {
    named(std::env::var("COMPUTER_SERVER_SANDBOXES").ok().as_deref())
}

fn named(listed: Option<&str>) -> Vec<String> {
    listed
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|place| !place.is_empty())
        .map(str::to_string)
        .collect()
}

fn listed(given: Option<&str>) -> Vec<String> {
    let found = named(given);

    if found.is_empty() {
        return vec!["docker".to_string()];
    }
    found
}

/// Somewhere a box might still be, and how to take one back from it.
///
/// A container runtime hands out one machine for every box on it. A vendor
/// cannot: its machine shares a cell with the profile that formats the box's
/// address, so two boxes taken back through one machine would each answer with
/// the other's URL. Hence a machine per box rather than per place.
pub enum Place {
    Runtime(String),
    Vendor(Arc<dyn RemoteApi>),
}

impl Place {
    fn name(&self) -> String {
        match self {
            Self::Runtime(runtime) => runtime.clone(),
            Self::Vendor(api) => api.vendor().to_string(),
        }
    }

    /// A machine to ask what is here. Which profile it is paired with does not
    /// reach a listing.
    fn asking(&self) -> Arc<dyn Machine> {
        self.driving(DisplayServer::default()).0
    }

    /// A machine and the profile that goes with it, for one box.
    fn driving(&self, server: DisplayServer) -> (Arc<dyn Machine>, Arc<dyn Profile>) {
        let image = spec::profile_for(server);

        match self {
            Self::Runtime(runtime) => (
                Arc::new(DockerMachine::new(Arc::new(SystemDocker::new(
                    runtime.clone(),
                )))),
                image,
            ),
            Self::Vendor(api) => {
                let (machine, profile) = remote::pair(Arc::clone(api), image);
                (Arc::new(machine), profile)
            }
        }
    }
}

/// The vendors this server was built to reach, in the order named.
///
/// A name it cannot serve is a warning and not a failure: a server asked for a
/// vendor it has no client or no credential for should still take back
/// everything else.
fn vendor(name: &str) -> Option<Arc<dyn RemoteApi>> {
    #[cfg(feature = "e2b")]
    if name == "e2b" {
        use computer::sandboxes::e2b::{E2bVendor, cloud::Cloud};

        return match Cloud::from_env() {
            Ok(cloud) => Some(Arc::new(E2bVendor::new(Arc::new(cloud)))),
            Err(error) => {
                tracing::warn!(vendor = %name, %error, "this vendor was named and cannot be reached");
                None
            }
        };
    }

    tracing::warn!(
        vendor = %name,
        "this vendor was named and is not built into this server"
    );
    None
}

/// Never fails: a runtime that is not installed is not an error at startup, and
/// a box that will not come back must not stop the ones that will.
pub async fn adopt(state: &AppState, runtimes: &[String], sandboxes: &[String]) -> usize {
    let mut places: Vec<Place> = runtimes.iter().cloned().map(Place::Runtime).collect();
    places.extend(
        sandboxes
            .iter()
            .filter_map(|name| vendor(name))
            .map(Place::Vendor),
    );

    adopt_from(state, &places).await
}

/// The places themselves rather than their names, so a vendor that answers from
/// a test can be handed in where one built from the environment cannot.
pub async fn adopt_from(state: &AppState, places: &[Place]) -> usize {
    let mut taken = 0;

    for place in places {
        let where_ = place.name();

        let found = match place.asking().labelled(BOX_LABEL).await {
            Ok(found) => found,
            Err(error) => {
                tracing::debug!(place = %where_, %error, "no boxes to take back from here");
                continue;
            }
        };

        for (name, value) in found {
            match adopt_one(state, place, &where_, &name, &value).await {
                Ok(()) => taken += 1,
                Err(error) => tracing::warn!(
                    box_ = %name,
                    place = %where_,
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
    state: &AppState,
    place: &Place,
    where_: &str,
    name: &str,
    value: &str,
) -> Result<(), String> {
    let Some(label) = BoxLabel::decode(value) else {
        return Err("its label is not one this server wrote".to_string());
    };

    let (machine, profile) = place.driving(label.spec.desktop.server);
    let computer = Computer::attach_using(machine, name, profile, None)
        .await
        .map_err(|error| error.to_string())?;

    let entry = state
        .registry
        .insert(
            name.to_string(),
            label.spec.clone(),
            label.screens,
            label.width,
            label.height,
            computer,
        )
        .await;

    // The record goes down again from the label rather than being read back:
    // the label is what the runtime still holds, and it is the only thing here
    // that outlived whichever server wrote the record first.
    let record = BoxRecord {
        id: entry.id.clone(),
        spec: label.spec.clone(),
        placement: label.placement.clone(),
        width: label.width,
        height: label.height,
        screens: label.screens,
        created_at_ms: crate::routes::ms_of(entry.created_at),
        expires_at_ms: None,
    };
    if let Err(why) = state.store.put_box(&record).await {
        tracing::warn!(box_ = %entry.id, %why, "an adopted box was not recorded");
    }

    state
        .record(
            &entry.id,
            Actor::System,
            TraceEvent::BoxCreated {
                spec_digest: label.digest,
                spec: Box::new(label.spec),
                placement: Box::new(label.placement),
                width: label.width,
                height: label.height,
                screens: label.screens,
            },
        )
        .await;
    state
        .record(
            &entry.id,
            Actor::System,
            TraceEvent::Adopted {
                runtime: where_.to_string(),
            },
        )
        .await;

    tracing::info!(box_ = %entry.id, place = %where_, "took a box back");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use computer::testing::ScriptedRemote;

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

    #[tokio::test]
    async fn test_a_box_in_a_vendor_is_taken_back() {
        let api = Arc::new(ScriptedRemote::new().holding("desk-1", "sbx-9"));
        api.metadata("sbx-9", BOX_LABEL, label().encode().expect("a label"));

        let state = AppState::default();
        let taken = adopt_from(&state, &[Place::Vendor(api)]).await;

        assert_eq!(taken, 1, "a box in somebody else's cloud comes back too");
        assert!(
            state.registry.get("desk-1").await.is_ok(),
            "and is drivable rather than only listed"
        );
        assert_eq!(
            state
                .store
                .get_box("desk-1")
                .await
                .expect("asked")
                .map(|held| held.width),
            Some(1280),
            "and its record is written from the label"
        );
    }

    #[tokio::test]
    async fn test_a_vendor_holding_nothing_this_server_wrote_takes_nothing() {
        let api = Arc::new(ScriptedRemote::new().holding("desk-1", "sbx-9"));

        let state = AppState::default();

        assert_eq!(
            adopt_from(&state, &[Place::Vendor(api)]).await,
            0,
            "a sandbox with no label of ours is somebody else's box"
        );
    }

    #[test]
    fn test_no_vendor_is_asked_unless_one_is_named() {
        assert!(named(None).is_empty());
        assert!(named(Some(" , ")).is_empty());
        assert_eq!(
            named(Some("e2b, daytona ,")),
            vec!["e2b".to_string(), "daytona".to_string()]
        );
    }

    #[test]
    fn test_a_vendor_this_server_cannot_serve_is_skipped() {
        assert!(vendor("nowhere").is_none());
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
