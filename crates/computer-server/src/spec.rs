//! Turning a [`Spec`] into the engine's builder, and into the numbers the
//! image actually gives it.
//!
//! This translation is the piece that moves. When the SDK can take a spec of
//! its own — `Builder::from_spec` — this module becomes a call into it, and
//! nothing else here changes. Keeping it in one file is what makes that a
//! move rather than a refactor.
//!
//! Defaults and limits live here rather than in the spec, because they belong
//! to an image: eight screens is what `computer-desktop` allows, not what a
//! desktop is.

use crate::error::ApiError;
use computer::{Auth, Bind, Builder, Computer, WaylandProfile, X11Profile};
use computer_spec::{self as spec, Placement, Spec};
use std::sync::Arc;
use std::time::Duration;

/// What a spec that left things open turns out to be, once an image has
/// answered for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
    pub width: u32,
    pub height: u32,
    pub screens: u32,
}

pub fn plan(
    spec: &Spec,
    placement: &Placement,
    name: &str,
) -> Result<(Builder, Resolved), ApiError> {
    let resolved = resolve(spec)?;

    let mut builder = Computer::builder()
        .name(name)
        // The box's lifetime belongs to this API, not to whichever request
        // happens to be holding a handle. Without this a dropped handle takes
        // a caller's box away mid-session.
        .keep_on_drop(true)
        .size(resolved.width, resolved.height)
        .network(spec.policy.network)
        .auth(auth_of(spec.policy.auth))
        .publish_on(bind_of(spec.policy.bind));

    builder = match spec.desktop.server {
        spec::DisplayServer::X11 => builder.profile(Arc::new(X11Profile)),
        spec::DisplayServer::Wayland => builder.profile(Arc::new(WaylandProfile)),
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

    Ok((builder, resolved))
}

pub fn resolve(spec: &Spec) -> Result<Resolved, ApiError> {
    if !spec.apps.is_empty() {
        let named: Vec<&str> = spec.apps.keys().map(String::as_str).collect();
        return Err(ApiError::bad_request(format!(
            "apps are not installable yet and this spec names {}: a box handed \
             back without them would look like the one that was asked for",
            named.join(", ")
        )));
    }

    let screens = spec.desktop.screens.unwrap_or(1);

    if screens == 0 {
        return Err(ApiError::bad_request(
            "a box has at least one screen: screen 0 starts with the desktop",
        ));
    }

    if screens > computer::image::MAX_SCREENS {
        return Err(ApiError::bad_request(format!(
            "the image supports {} screens and this spec asks for {screens}",
            computer::image::MAX_SCREENS
        )));
    }

    Ok(Resolved {
        width: spec.desktop.width.unwrap_or(computer::image::WIDTH),
        height: spec.desktop.height.unwrap_or(computer::image::HEIGHT),
        screens,
    })
}

fn packages_for(feature: spec::Feature) -> Vec<String> {
    use computer::bundle::Extras;

    match feature {
        spec::Feature::WideFonts => Extras::wide_fonts().packages,
        spec::Feature::Audio => Extras::audio().packages,
        spec::Feature::Video => Extras::video().packages,
        spec::Feature::Dock => Extras::dock().packages,
    }
}

fn auth_of(auth: spec::Auth) -> Auth {
    match auth {
        spec::Auth::None => Auth::Open,
        spec::Auth::Password => Auth::Password,
        spec::Auth::Token => Auth::Token,
    }
}

fn bind_of(bind: spec::Bind) -> Bind {
    match bind {
        spec::Bind::Loopback => Bind::Loopback,
        spec::Bind::Any => Bind::Any,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_spec_that_names_no_size_takes_the_image_s() {
        let resolved = resolve(&Spec::default()).expect("a spec that says nothing is launchable");

        assert_eq!(resolved.width, computer::image::WIDTH);
        assert_eq!(resolved.height, computer::image::HEIGHT);
        assert_eq!(resolved.screens, 1);
    }

    #[test]
    fn test_a_spec_naming_apps_is_refused() {
        let spec: Spec = serde_json::from_str(r#"{"apps":{"vscode":{}}}"#).unwrap();

        let Err(error) = resolve(&spec) else {
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

        assert!(resolve(&spec).is_err());
    }

    #[test]
    fn test_a_box_with_no_screens_is_refused() {
        let spec: Spec = serde_json::from_str(r#"{"desktop":{"screens":0}}"#).unwrap();

        assert!(resolve(&spec).is_err());
    }
}
