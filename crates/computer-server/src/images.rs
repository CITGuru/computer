use crate::AppState;
use crate::error::ApiError;
use crate::runtimes::{Place, Runtime};
use crate::spec::Resolved;
use computer::{Computer, Config, Machine};
use computer_storage::ImageRecord;
use computer_types::{Placement, Spec};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub async fn started(
    state: &AppState,
    spec: &Spec,
    placement: &Placement,
    id: &str,
    runtime: &Runtime,
) -> Result<(Computer, Resolved), ApiError> {
    let digest = spec.digest();
    let recorded = known(state, runtime, &digest).await;

    let (computer, resolved, started_from) =
        match launch(spec, placement, id, runtime, recorded.clone()).await {
            Ok(started) => started,
            Err(why) if recorded.is_some() => {
                tracing::warn!(
                    runtime = %runtime.name,
                    %digest,
                    why = %why.body.message,
                    "the recorded image did not start a box; building it again"
                );
                forget(state, &runtime.name, &digest).await;
                launch(spec, placement, id, runtime, None).await?
            }
            Err(why) => return Err(why),
        };

    keep(state, &runtime.name, &digest, &started_from).await;
    Ok((computer, resolved))
}

async fn launch(
    spec: &Spec,
    placement: &Placement,
    id: &str,
    runtime: &Runtime,
    image: Option<String>,
) -> Result<(Computer, Resolved, String), ApiError> {
    let (mut builder, resolved) = crate::spec::plan(spec, placement, id, runtime)?;
    if let Some(image) = image {
        builder = builder.image(image);
    }

    let config = builder.config()?;
    let computer = builder.launch().await?;
    let started_from = used(computer.machine(), &config);

    Ok((computer, resolved, started_from))
}

pub async fn known(state: &AppState, runtime: &Runtime, digest: &str) -> Option<String> {
    if !matches!(runtime.place, Place::Remote { .. }) {
        return None;
    }

    match state.store.get_image(&runtime.name, digest).await {
        Ok(found) => found.map(|record| record.reference),
        Err(why) => {
            tracing::warn!(
                runtime = %runtime.name,
                %digest,
                %why,
                "the image manifest is unreadable"
            );
            None
        }
    }
}

pub fn used(machine: &Arc<dyn Machine>, config: &Config) -> String {
    machine
        .image_used(config)
        .unwrap_or_else(|| config.image.clone())
}

pub async fn keep(state: &AppState, runtime: &str, digest: &str, reference: &str) {
    let record = ImageRecord {
        runtime: runtime.to_string(),
        spec_digest: digest.to_string(),
        reference: reference.to_string(),
        built_at_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_millis() as u64)
            .unwrap_or_default(),
        bytes: None,
    };

    if let Err(why) = state.store.put_image(&record).await {
        tracing::warn!(%runtime, %digest, %why, "the image was built but not recorded");
    }
}

pub async fn forget(state: &AppState, runtime: &str, digest: &str) {
    if let Err(why) = state.store.forget_image(runtime, digest).await {
        tracing::warn!(%runtime, %digest, %why, "a stale image record stays in the manifest");
    }
}
