use crate::error::ApiError;
use crate::recover::{BOX_LABEL, BoxLabel};
use computer::Builder;
use computer_types::{Placement, Spec};

pub use computer::spec::{Resolved, profile_for, resolve};

pub fn plan(
    spec: &Spec,
    placement: &Placement,
    name: &str,
) -> Result<(Builder, Resolved), ApiError> {
    let resolved = resolve(spec)?;

    let mut builder = Builder::from_spec(spec)?
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

    #[test]
    fn test_a_spec_the_engine_refuses_is_a_bad_request() {
        let spec: Spec = serde_json::from_str(r#"{"desktop":{"screens":99}}"#).unwrap();

        let Err(error) = plan(&spec, &Placement::default(), "box") else {
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

        let Err(error) = plan(&Spec::default(), &placement, "box") else {
            panic!("a box was accepted that would be removed while starting");
        };
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_a_plan_carries_the_spec_back_in_a_label() {
        let (_, resolved) =
            plan(&Spec::default(), &Placement::default(), "box").expect("a default spec launches");

        assert_eq!(resolved.screens, 1);
    }
}
