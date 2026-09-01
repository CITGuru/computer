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
//! desktop is. They are asked of the profile the spec selects rather than read
//! off one image's constants — `MacProfile` allows a single screen, because
//! macOS has one GUI session per boot.

use crate::error::ApiError;
use computer::{Auth, Bind, Builder, Computer, Profile, WaylandProfile, X11Profile};
use computer_types::{self as spec, Placement, Spec};
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
    let profile = profile_for(spec.desktop.server);
    let resolved = resolve_with(profile.as_ref(), spec)?;

    let mut builder = Computer::builder()
        .name(name)
        // The box's lifetime belongs to this API, not to whichever request
        // happens to be holding a handle. Without this a dropped handle takes
        // a caller's box away mid-session.
        .keep_on_drop(true)
        .size(resolved.width, resolved.height)
        .network(spec.policy.network)
        .auth(auth_of(spec.policy.auth))
        .publish_on(bind_of(spec.policy.bind))
        .profile(Arc::clone(&profile));

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
    resolve_with(profile_for(spec.desktop.server).as_ref(), spec)
}

fn profile_for(server: spec::DisplayServer) -> Arc<dyn Profile> {
    match server {
        spec::DisplayServer::X11 => Arc::new(X11Profile),
        spec::DisplayServer::Wayland => Arc::new(WaylandProfile),
    }
}

fn resolve_with(profile: &dyn Profile, spec: &Spec) -> Result<Resolved, ApiError> {
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

    let most = profile.ports().max_screens;
    if screens > most {
        return Err(ApiError::bad_request(format!(
            "the {} image runs {most} screen(s) and this spec asks for {screens}",
            profile.name()
        )));
    }

    let (width, height) = profile.default_size();

    Ok(Resolved {
        width: spec.desktop.width.unwrap_or(width),
        height: spec.desktop.height.unwrap_or(height),
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
    fn test_a_spec_that_names_no_size_takes_the_profile_s() {
        let resolved = resolve(&Spec::default()).expect("a spec that says nothing is launchable");
        let (width, height) = X11Profile.default_size();

        assert_eq!((resolved.width, resolved.height), (width, height));
        assert_eq!(resolved.screens, 1);
    }

    #[test]
    fn test_a_wayland_spec_resolves_against_the_wayland_profile() {
        let spec: Spec = serde_json::from_str(r#"{"desktop":{"server":"wayland"}}"#).unwrap();
        let resolved = resolve(&spec).expect("a wayland box is launchable");

        assert_eq!(
            (resolved.width, resolved.height),
            WaylandProfile.default_size()
        );
    }

    #[test]
    fn test_the_screen_limit_comes_from_the_profile_rather_than_one_image() {
        let spec: Spec = serde_json::from_str(r#"{"desktop":{"screens":99}}"#).unwrap();

        let Err(error) = resolve(&spec) else {
            panic!("a spec asking for more screens than the image runs was accepted");
        };
        // Naming the image is the difference between a caller fixing their
        // spec and a caller filing a bug.
        assert!(
            error.body.message.contains(X11Profile.name()),
            "the refusal names the image that refused: {}",
            error.body.message
        );
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
