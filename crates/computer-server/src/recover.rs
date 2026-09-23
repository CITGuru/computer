use crate::AppState;
use crate::runtimes::Runtime;
use computer::Computer;
use computer_api::{Actor, Button, Placement, RuntimeState, Spec, TraceEvent};
use computer_storage::BoxRecord;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

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
    pub fn encode(&self) -> Option<String> {
        serde_json::to_string(self).ok()
    }

    pub fn decode(value: &str) -> Option<Self> {
        serde_json::from_str(value).ok()
    }

    pub fn of(record: &BoxRecord) -> Self {
        Self {
            digest: record.spec.digest(),
            spec: record.spec.clone(),
            placement: record.placement.clone(),
            width: record.width,
            height: record.height,
            screens: record.screens,
        }
    }
}

pub async fn adopt(state: &AppState) -> usize {
    from_records(state).await + from_labels(state).await
}

pub async fn from_records(state: &AppState) -> usize {
    let records = match state.store.list_boxes().await {
        Ok(records) => records,
        Err(why) => {
            tracing::warn!(%why, "the boxes this server recorded could not be read");
            return 0;
        }
    };

    let mut taken = 0;

    for record in records {
        if state.registry.get(&record.id).await.is_ok() {
            continue;
        }

        let runtime = match state.runtimes.get(&record.runtime) {
            Some(runtime) => runtime,
            None => {
                state.out_of_reach(
                    &record.id,
                    format!("its runtime {} is not on this server", record.runtime),
                );
                continue;
            }
        };

        if let RuntimeState::Unavailable { why } = &runtime.state {
            state.out_of_reach(
                &record.id,
                format!("its runtime {} is not answering: {why}", record.runtime),
            );
            continue;
        }

        match take(
            state,
            &runtime,
            &record.id,
            &BoxLabel::of(&record),
            Some(&record),
        )
        .await
        {
            Ok(()) => taken += 1,
            Err(why) => tracing::debug!(
                box_ = %record.id,
                runtime = %record.runtime,
                %why,
                "a recorded box is no longer there"
            ),
        }
    }

    taken
}

pub async fn from_labels(state: &AppState) -> usize {
    let mut taken = 0;

    for runtime in state.runtimes.all() {
        if !runtime.ready() {
            continue;
        }

        let found = match runtime.scanning().labelled(BOX_LABEL).await {
            Ok(found) => found,
            Err(error) => {
                tracing::debug!(runtime = %runtime.name, %error, "no boxes to take back from here");
                continue;
            }
        };

        for (name, value) in found {
            if state.registry.get(&name).await.is_ok() {
                continue;
            }

            let Some(label) = BoxLabel::decode(&value) else {
                tracing::warn!(
                    box_ = %name,
                    runtime = %runtime.name,
                    "a box is here under a label this server did not write"
                );
                continue;
            };

            match take(state, &runtime, &name, &label, None).await {
                Ok(()) => taken += 1,
                Err(why) => tracing::warn!(
                    box_ = %name,
                    runtime = %runtime.name,
                    %why,
                    "a box is here that this server could not take back; it will \
                     keep what it holds until something else removes it"
                ),
            }
        }
    }

    taken
}

async fn take(
    state: &AppState,
    runtime: &Runtime,
    name: &str,
    label: &BoxLabel,
    recorded: Option<&BoxRecord>,
) -> Result<(), String> {
    let (machine, profile) = runtime.pair(label.spec.desktop.server);

    // A paused box reports no ports, so it is woken long enough to read them.
    let frozen = machine.paused(name).await.unwrap_or(false);
    if frozen {
        machine
            .resume(name)
            .await
            .map_err(|error| format!("it is paused and would not wake to be read: {error}"))?;
    }

    // Stopped boxes too, or they stay on disk with nothing that can start them.
    let running = machine.running(name).await.unwrap_or(false);
    let taken = match running {
        true => Computer::attach_using(Arc::clone(&machine), name, profile, None).await,
        false => Computer::attach_stopped(Arc::clone(&machine), name, profile, None).await,
    };

    if frozen {
        let _ = machine.pause(name).await;
    }

    let mut computer = taken.map_err(|error| error.to_string())?;
    computer.expires_when(
        recorded
            .and_then(|record| record.expires_at_ms)
            .map(|at| UNIX_EPOCH + Duration::from_millis(at)),
    );

    if running && !frozen {
        for button in [Button::Left, Button::Middle, Button::Right] {
            let _ = computer::Desktop::let_go(&computer, button).await;
        }
        let _ = computer::Desktop::let_keys_go(&computer).await;
    }

    let entry = state
        .registry
        .insert(
            name.to_string(),
            runtime.name.clone(),
            label.spec.clone(),
            computer::spec::Resolved {
                width: label.width,
                height: label.height,
                screens: label.screens,
            },
            computer,
        )
        .await;

    if recorded.is_none() {
        let record = BoxRecord {
            id: entry.id.clone(),
            runtime: runtime.name.clone(),
            spec: label.spec.clone(),
            placement: label.placement.clone(),
            width: label.width,
            height: label.height,
            screens: label.screens,
            created_at_ms: crate::routes::ms_of(entry.created_at),
            expires_at_ms: entry.computer.expires_at().map(crate::routes::ms_of),
        };
        if let Err(why) = state.store.put_box(&record).await {
            tracing::warn!(box_ = %entry.id, %why, "an adopted box was not recorded");
        }

        state
            .record(
                &entry.id,
                Actor::System,
                TraceEvent::BoxCreated {
                    spec_digest: label.digest.clone(),
                    spec: Box::new(label.spec.clone()),
                    placement: Box::new(label.placement.clone()),
                    width: label.width,
                    height: label.height,
                    screens: label.screens,
                },
            )
            .await;
    }

    state
        .record(
            &entry.id,
            Actor::System,
            TraceEvent::Adopted {
                runtime: runtime.name.clone(),
            },
        )
        .await;

    tracing::info!(box_ = %entry.id, runtime = %runtime.name, "took a box back");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtimes::{self, Runtimes};
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

    fn holding(api: Arc<ScriptedRemote>) -> AppState {
        let mut runtimes = Runtimes::default();
        runtimes.add(runtimes::remote(
            "cloud".to_string(),
            api,
            runtimes::Tuning::default(),
        ));
        runtimes.settle();

        AppState::default().with(runtimes)
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

        let state = holding(api);
        let taken = adopt(&state).await;

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
                .map(|held| held.runtime),
            Some("cloud".to_string()),
            "and its record says where it was found, so a restart goes there first"
        );
    }

    #[tokio::test]
    async fn test_a_vendor_holding_nothing_this_server_wrote_takes_nothing() {
        let api = Arc::new(ScriptedRemote::new().holding("desk-1", "sbx-9"));

        let state = holding(api);

        assert_eq!(
            adopt(&state).await,
            0,
            "a sandbox with no label of ours is somebody else's box"
        );
    }

    #[tokio::test]
    async fn test_a_recorded_box_is_taken_back_through_the_runtime_it_names() {
        let api = Arc::new(ScriptedRemote::new().holding("desk-1", "sbx-9"));
        let state = holding(api);

        state
            .store
            .put_box(&BoxRecord {
                id: "desk-1".to_string(),
                runtime: "cloud".to_string(),
                spec: Spec::default(),
                placement: Placement::default(),
                width: 1280,
                height: 800,
                screens: 1,
                created_at_ms: 1_700_000_000_000,
                expires_at_ms: None,
            })
            .await
            .expect("recorded");

        assert_eq!(adopt(&state).await, 1, "the record alone brings it back");
        assert_eq!(
            state
                .store
                .get_box("desk-1")
                .await
                .expect("asked")
                .map(|held| held.created_at_ms),
            Some(1_700_000_000_000),
            "and the record it came from is left as it was"
        );
    }

    #[tokio::test]
    async fn test_a_box_comes_back_with_the_deadline_it_had() {
        let api = Arc::new(ScriptedRemote::new().holding("desk-1", "sbx-9"));
        let state = holding(api);
        let ends = crate::routes::ms_of(std::time::SystemTime::now()) + 30 * 60 * 1000;

        state
            .store
            .put_box(&BoxRecord {
                id: "desk-1".to_string(),
                runtime: "cloud".to_string(),
                spec: Spec::default(),
                placement: Placement::default(),
                width: 1280,
                height: 800,
                screens: 1,
                created_at_ms: 1_700_000_000_000,
                expires_at_ms: Some(ends),
            })
            .await
            .expect("recorded");

        assert_eq!(adopt(&state).await, 1);

        let entry = state.registry.get("desk-1").await.expect("it came back");
        assert_eq!(
            entry.computer.expires_at().map(crate::routes::ms_of),
            Some(ends),
            "a box that outlives the server still ends when it was going to"
        );
    }

    #[tokio::test]
    async fn test_a_box_whose_runtime_is_gone_is_out_of_reach_rather_than_forgotten() {
        let state = AppState::default();

        state
            .store
            .put_box(&BoxRecord {
                id: "desk-1".to_string(),
                runtime: "cloud".to_string(),
                spec: Spec::default(),
                placement: Placement::default(),
                width: 1280,
                height: 800,
                screens: 1,
                created_at_ms: 1_700_000_000_000,
                expires_at_ms: None,
            })
            .await
            .expect("recorded");

        assert_eq!(adopt(&state).await, 0);
        assert!(
            state.why_out_of_reach("desk-1").is_some(),
            "a box on a runtime this server no longer has is reported, not dropped"
        );
        assert!(
            state
                .store
                .get_box("desk-1")
                .await
                .expect("asked")
                .is_some(),
            "and its record stays, so it comes back when the runtime does"
        );
    }
}
