//! Turning a [`Spec`] into the engine's builder, and naming what it asked for.
//!
//! This translation is the piece that moves. When the SDK can take a spec of
//! its own — `Builder::from_spec` — this module becomes a call into it, and
//! nothing else here changes. Keeping it in one file is what makes that a
//! move rather than a refactor.

use crate::error::ApiError;
use crate::wire::{self, Placement, Spec};
use computer::{Auth, Bind, Builder, Computer, WaylandProfile, X11Profile};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;

/// Two callers asking for the same desktop get the same digest, whatever
/// order they wrote the keys in.
pub fn digest(spec: &Spec) -> String {
    // Through `serde_json::Value`, whose maps are ordered, so the digest
    // follows the spec rather than the request body's formatting.
    let canonical = serde_json::to_value(spec)
        .and_then(|value| serde_json::to_string(&value))
        .unwrap_or_default();

    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn builder_for(spec: &Spec, placement: &Placement, name: &str) -> Result<Builder, ApiError> {
    if !spec.apps.is_empty() {
        let named: Vec<&str> = spec.apps.keys().map(String::as_str).collect();
        return Err(ApiError::bad_request(format!(
            "apps are not installable yet and this spec names {}: a box handed \
             back without them would look like the one that was asked for",
            named.join(", ")
        )));
    }

    if spec.desktop.screens == 0 {
        return Err(ApiError::bad_request(
            "a box has at least one screen: screen 0 starts with the desktop",
        ));
    }

    if spec.desktop.screens > computer::image::MAX_SCREENS {
        return Err(ApiError::bad_request(format!(
            "the image supports {} screens and this spec asks for {}",
            computer::image::MAX_SCREENS,
            spec.desktop.screens
        )));
    }

    let mut builder = Computer::builder()
        .name(name)
        // The box's lifetime belongs to this API, not to whichever request
        // happens to be holding a handle. Without this a dropped handle takes
        // a caller's box away mid-session.
        .keep_on_drop(true)
        .size(spec.desktop.width, spec.desktop.height)
        .network(spec.policy.network)
        .auth(auth_of(spec.policy.auth))
        .publish_on(bind_of(spec.policy.bind));

    builder = match spec.desktop.server {
        wire::DisplayServer::X11 => builder.profile(Arc::new(X11Profile)),
        wire::DisplayServer::Wayland => builder.profile(Arc::new(WaylandProfile)),
    };

    let mut packages: Vec<String> = spec.desktop.packages.clone();
    for feature in &spec.desktop.features {
        packages.extend(packages_for(*feature));
    }
    if !packages.is_empty() {
        builder = builder.packages(packages);
    }

    if let Some(host) = &spec.policy.advertise {
        builder = builder.advertise(host.clone());
    }

    if let Some(runtime) = &placement.runtime {
        builder = builder.runtime(runtime.clone());
    }
    if let Some(memory) = &placement.memory {
        builder = builder.memory(memory.clone());
    }
    if let Some(cpus) = &placement.cpus {
        builder = builder.cpus(cpus.clone());
    }
    if let Some(secs) = placement.expires_after_secs {
        builder = builder.expires_after(Duration::from_secs(secs));
    }
    if let Some(secs) = placement.idle_timeout_secs {
        builder = builder.expires_when_idle(Duration::from_secs(secs));
    }

    Ok(builder)
}

fn packages_for(feature: wire::Feature) -> Vec<String> {
    use computer::bundle::Extras;

    match feature {
        wire::Feature::WideFonts => Extras::wide_fonts().packages,
        wire::Feature::Audio => Extras::audio().packages,
        wire::Feature::Video => Extras::video().packages,
        wire::Feature::Dock => Extras::dock().packages,
    }
}

fn auth_of(auth: wire::Auth) -> Auth {
    match auth {
        wire::Auth::None => Auth::Open,
        wire::Auth::Password => Auth::Password,
        wire::Auth::Token => Auth::Token,
    }
}

fn bind_of(bind: wire::Bind) -> Bind {
    match bind {
        wire::Bind::Loopback => Bind::Loopback,
        wire::Bind::Any => Bind::Any,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_digest_follows_the_spec_not_the_formatting() {
        let one: Spec = serde_json::from_str(r#"{"desktop":{"width":800,"height":600}}"#).unwrap();
        let two: Spec = serde_json::from_str(r#"{"desktop":{"height":600,"width":800}}"#).unwrap();

        assert_eq!(digest(&one), digest(&two));
    }

    #[test]
    fn test_a_different_desktop_is_a_different_digest() {
        let one = Spec::default();
        let mut two = Spec::default();
        two.desktop.width += 1;

        assert_ne!(digest(&one), digest(&two));
    }

    #[test]
    fn test_a_spec_naming_apps_is_refused() {
        let spec: Spec = serde_json::from_str(r#"{"apps":{"vscode":{}}}"#).unwrap();

        let Err(error) = builder_for(&spec, &Placement::default(), "box") else {
            panic!("a spec naming an app it cannot get was accepted");
        };
        assert!(
            error.body.message.contains("vscode"),
            "the refusal names what it could not install: {}",
            error.body.message
        );
    }

    #[test]
    fn test_more_screens_than_the_image_has_is_refused() {
        let spec: Spec = serde_json::from_str(r#"{"desktop":{"screens":99}}"#).unwrap();

        assert!(builder_for(&spec, &Placement::default(), "box").is_err());
    }

    #[test]
    fn test_a_misspelled_key_is_refused_rather_than_ignored() {
        assert!(serde_json::from_str::<Spec>(r#"{"desktop":{"widht":800}}"#).is_err());
    }
}
