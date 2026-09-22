use crate::error::ApiError;
use crate::recover::{BOX_LABEL, BoxLabel};
use crate::runtimes::Runtime;
use computer::Builder;
use computer_types::{Placement, Spec};

pub use computer::spec::{Resolved, profile_for, resolve};

pub fn plan(
    spec: &Spec,
    placement: &Placement,
    name: &str,
    runtime: &Runtime,
) -> Result<(Builder, Resolved), ApiError> {
    let resolved = resolve(spec)?;

    runtime.check(placement)?;

    let mut builder = runtime
        .drive(Builder::from_spec(spec)?, spec.desktop.server)
        .place(placement)?
        .name(name)
        // The box outlives the request, so a dropped handle must not take it away.
        .keep_on_drop(true);

    let label = BoxLabel {
        digest: spec.digest(),
        spec: spec.clone(),
        placement: placement.clone(),
        width: resolved.width,
        height: resolved.height,
        screens: resolved.screens,
    };
    if let Some(value) = label.encode() {
        builder = builder.label(BOX_LABEL, value);
    }

    Ok((builder, resolved))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use computer::DockerMachine;
    use computer::testing::ScriptedCli;
    use std::sync::Arc;

    fn on_docker() -> Runtime {
        crate::runtimes::engine(
            "docker",
            Arc::new(DockerMachine::new(Arc::new(ScriptedCli::new()))),
        )
    }

    #[test]
    fn test_a_spec_the_engine_refuses_is_a_bad_request() {
        let spec: Spec = serde_json::from_str(r#"{"desktop":{"screens":99}}"#).unwrap();

        let Err(error) = plan(&spec, &Placement::default(), "box", &on_docker()) else {
            panic!("a spec asking for more screens than the image runs was accepted");
        };
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_a_life_too_short_is_a_bad_request() {
        let placement = Placement {
            expires_after_secs: Some(8),
            ..Placement::default()
        };

        let Err(error) = plan(&Spec::default(), &placement, "box", &on_docker()) else {
            panic!("a box was accepted that would be removed while starting");
        };
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_what_a_runtime_gives_every_box_is_a_default_the_placement_overrides() {
        let mut tuned = on_docker();
        tuned.tuning.memory = Some("4g".to_string());

        let (builder, _) =
            plan(&Spec::default(), &Placement::default(), "box", &tuned).expect("a box");
        assert_eq!(
            builder.config().expect("a config").memory.as_deref(),
            Some("4g")
        );

        let placement = Placement {
            memory: Some("8g".to_string()),
            ..Placement::default()
        };
        let (builder, _) = plan(&Spec::default(), &placement, "box", &tuned).expect("a box");
        assert_eq!(
            builder.config().expect("a config").memory.as_deref(),
            Some("8g"),
            "what the caller asked for wins over what the runtime gives by default"
        );
    }

    #[test]
    fn test_an_engine_offered_on_another_oci_runtime_starts_its_boxes_there() {
        let mut hardened = on_docker();
        hardened.tuning.isolation = Some("runsc".to_string());

        let (builder, _) =
            plan(&Spec::default(), &Placement::default(), "box", &hardened).expect("a box");
        let args = builder.preview().expect("the command it would run");

        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "--runtime" && pair[1] == "runsc"),
            "the box goes on the OCI runtime the file named, not the engine's default: {args:?}"
        );
    }

    #[test]
    fn test_a_plan_carries_the_spec_back_in_a_label() {
        let (_, resolved) = plan(&Spec::default(), &Placement::default(), "box", &on_docker())
            .expect("a default spec launches");

        assert_eq!(resolved.screens, 1);
    }
}
