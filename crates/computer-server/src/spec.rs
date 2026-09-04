//! The server's half of launching from a spec.
//!
//! Translation lives in the engine — [`computer::spec`] — so a caller with the
//! SDK and no server gets the same box from the same spec. What is added here
//! belongs to an API rather than to a desktop: an id for a name, a lifetime
//! that outlives the request holding the handle, and the label a restart reads
//! the whole spec back from.

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
        // The box's lifetime belongs to this API, not to whichever request
        // happens to be holding a handle. Without this a dropped handle takes
        // a caller's box away mid-session.
        .keep_on_drop(true);

    // Written on the box rather than held in this process, so a restart can
    // find it again and know what it was.
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
