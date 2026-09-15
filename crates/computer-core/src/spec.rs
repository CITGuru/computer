use crate::apps;
use crate::bundle::{self, Extras};
use crate::{Auth, Bind, Builder, Computer, Error, Profile, Result, WaylandProfile, X11Profile};
use computer_types as spec;
use computer_types::{Placement, Spec};
use std::sync::Arc;
use std::time::Duration;

/// The clock starts at creation, and below this a box can expire while starting.
const MIN_LIFE: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
    pub width: u32,
    pub height: u32,
    pub screens: u32,
}

pub fn resolve(spec: &Spec) -> Result<Resolved> {
    resolve_with(profile_for(spec.desktop.server).as_ref(), spec)
}

pub fn profile_for(server: spec::DisplayServer) -> Arc<dyn Profile> {
    match server {
        spec::DisplayServer::X11 => Arc::new(X11Profile),
        spec::DisplayServer::Wayland => Arc::new(WaylandProfile),
    }
}

impl Builder {
    pub fn from_spec(spec: &Spec) -> Result<Self> {
        let profile = profile_for(spec.desktop.server);
        let resolved = resolve_with(profile.as_ref(), spec)?;

        let mut builder = Computer::builder()
            .size(resolved.width, resolved.height)
            .network(spec.policy.network)
            .auth(auth_of(spec.policy.auth))
            .publish_on(bind_of(spec.policy.bind))
            .profile(profile);

        let mut packages = spec.desktop.packages.clone();
        for feature in &spec.desktop.features {
            packages.extend(packages_for(*feature));
        }

        let mut sources = Vec::new();
        let mut launchers = Vec::new();

        for (name, app) in apps::resolve_all(spec)? {
            packages.extend(app.packages);

            if let Some(source) = app.source {
                sources.push(bundle::AptSource {
                    name: name.clone(),
                    key_url: source.key_url,
                    list: source.list,
                });
            }

            // The dock needs a class to find an open window and a command to start one.
            if let (Some(spec::WindowMatch::Class(class)), false) =
                (app.window.clone(), app.command.is_empty())
            {
                launchers.push(bundle::Launcher {
                    name,
                    class,
                    command: app.command,
                });
            }
        }

        if !packages.is_empty() || !sources.is_empty() {
            builder = builder
                .packages_from(packages, sources)
                .launchers(launchers);
        }

        if let Some(host) = &spec.policy.advertise {
            builder = builder.advertise(host.clone());
        }

        Ok(builder)
    }

    pub fn place(mut self, placement: &Placement) -> Result<Self> {
        placeable(placement)?;

        if let Some(runtime) = &placement.runtime {
            self = self.runtime(runtime.clone());
        }
        if let Some(memory) = &placement.memory {
            self = self.memory(memory.clone());
        }
        if let Some(cpus) = &placement.cpus {
            self = self.cpus(cpus.clone());
        }
        if let Some(secs) = placement.expires_after_secs {
            self = self.expires_after(Duration::from_secs(secs));
        }
        if let Some(secs) = placement.idle_timeout_secs {
            self = self.expires_when_idle(Duration::from_secs(secs));
        }

        Ok(self)
    }
}

fn resolve_with(profile: &dyn Profile, spec: &Spec) -> Result<Resolved> {
    // An unknown app is refused here too, not only when something builds it.
    apps::resolve_all(spec)?;

    let screens = spec.desktop.screens.unwrap_or(1);

    if screens == 0 {
        return Err(Error::invalid(
            "a box has at least one screen: screen 0 starts with the desktop",
        ));
    }

    let most = profile.ports().max_screens;
    if screens > most {
        return Err(Error::invalid(format!(
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

fn placeable(placement: &Placement) -> Result<()> {
    for (secs, what) in [
        (placement.expires_after_secs, "expires_after_secs"),
        (placement.idle_timeout_secs, "idle_timeout_secs"),
    ] {
        if let Some(secs) = secs
            && secs < MIN_LIFE
        {
            return Err(Error::invalid(format!(
                "{what} of {secs}s is shorter than a box takes to come up. The clock starts when the box is created, not when it is ready, so it would be removed mid-launch and the wait would end in a timeout. {MIN_LIFE}s is the shortest that works."
            )));
        }
    }

    Ok(())
}

fn packages_for(feature: spec::Feature) -> Vec<String> {
    match feature {
        spec::Feature::WideFonts => Extras::wide_fonts().packages,
        spec::Feature::Audio => Extras::audio().packages,
        spec::Feature::Video => Extras::video().packages,
        spec::Feature::Dock => Extras::dock().packages,
        spec::Feature::X11Apps => Extras::x11_apps().packages,
        spec::Feature::Accessibility => Extras::accessibility().packages,
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

    fn parsed(json: &str) -> Spec {
        serde_json::from_str(json).expect("a spec")
    }

    #[test]
    fn test_a_spec_that_names_no_size_takes_the_profile_s() {
        let resolved = resolve(&Spec::default()).expect("a spec that says nothing is launchable");

        assert_eq!((resolved.width, resolved.height), X11Profile.default_size());
        assert_eq!(resolved.screens, 1);
    }

    #[test]
    fn test_a_wayland_spec_resolves_against_the_wayland_profile() {
        let resolved =
            resolve(&parsed(r#"{"desktop":{"server":"wayland"}}"#)).expect("a wayland box");

        assert_eq!(
            (resolved.width, resolved.height),
            WaylandProfile.default_size()
        );
    }

    #[test]
    fn test_a_size_the_spec_names_survives_resolution() {
        let resolved = resolve(&parsed(r#"{"desktop":{"width":1600,"height":1200}}"#))
            .expect("a spec that pins its size");

        assert_eq!((resolved.width, resolved.height), (1600, 1200));
    }

    #[test]
    fn test_the_screen_limit_comes_from_the_profile_rather_than_one_image() {
        let Err(error) = resolve(&parsed(r#"{"desktop":{"screens":99}}"#)) else {
            panic!("a spec asking for more screens than the image runs was accepted");
        };

        assert!(
            error.to_string().contains(X11Profile.name()),
            "the refusal names the image that refused: {error}"
        );
    }

    #[test]
    fn test_a_spec_naming_an_app_the_catalog_holds_is_accepted() {
        resolve(&parsed(r#"{"apps":{"vscode":{}}}"#)).expect("a name this crate knows");
    }

    #[test]
    fn test_a_spec_naming_an_app_nothing_can_install_is_refused() {
        let Err(error) = resolve(&parsed(r#"{"apps":{"gimpp":{}}}"#)) else {
            panic!("a spec naming an app it cannot get was accepted");
        };

        assert!(
            error.to_string().contains("gimpp"),
            "the refusal names what it could not install: {error}"
        );
    }

    #[test]
    fn test_an_apps_packages_and_source_reach_the_build() {
        let builder = Builder::from_spec(&parsed(r#"{"apps":{"vscode":{}}}"#)).expect("a builder");
        let config = builder.config().expect("the image it would build");

        assert!(
            config.extras.packages.contains(&"code".to_string()),
            "the app's package is installed: {:?}",
            config.extras.packages
        );
        assert_eq!(
            config.extras.sources.len(),
            1,
            "VS Code is not in Debian, so its archive travels with it"
        );
    }

    #[test]
    fn test_one_app_list_against_two_archives_is_two_images() {
        let plain = Builder::from_spec(&parsed(r#"{"desktop":{"packages":["code"]}}"#))
            .expect("a builder")
            .config()
            .expect("its image");
        let sourced = Builder::from_spec(&parsed(r#"{"apps":{"vscode":{}}}"#))
            .expect("a builder")
            .config()
            .expect("its image");

        assert_ne!(
            plain.image, sourced.image,
            "a package name means something different once its archive is added"
        );
    }

    #[test]
    fn test_a_box_with_no_screens_is_refused() {
        assert!(resolve(&parsed(r#"{"desktop":{"screens":0}}"#)).is_err());
    }

    #[test]
    fn test_a_refused_spec_is_the_caller_s_to_fix() {
        let Err(error) = resolve(&parsed(r#"{"desktop":{"screens":99}}"#)) else {
            panic!("accepted");
        };

        assert!(
            !error.needs_another_place(),
            "no other image takes this spec either"
        );
    }

    #[test]
    fn test_a_life_too_short_to_start_in_is_refused() {
        let placement = Placement {
            expires_after_secs: Some(8),
            ..Placement::default()
        };

        let Err(error) = Builder::from_spec(&Spec::default())
            .expect("a default spec builds")
            .place(&placement)
        else {
            panic!("a box was accepted that would be removed while starting");
        };

        assert!(
            error.to_string().contains("expires_after_secs"),
            "the refusal names the field: {error}"
        );
    }

    #[test]
    fn test_an_idle_timeout_too_short_is_refused_the_same_way() {
        let placement = Placement {
            idle_timeout_secs: Some(1),
            ..Placement::default()
        };

        assert!(
            Builder::from_spec(&Spec::default())
                .expect("a default spec builds")
                .place(&placement)
                .is_err()
        );
    }

    #[test]
    fn test_a_life_long_enough_is_allowed() {
        let placement = Placement {
            expires_after_secs: Some(MIN_LIFE),
            ..Placement::default()
        };

        assert!(
            Builder::from_spec(&Spec::default())
                .expect("a default spec builds")
                .place(&placement)
                .is_ok()
        );
    }

    #[test]
    fn test_a_feature_becomes_the_packages_that_serve_it() {
        assert_eq!(
            packages_for(spec::Feature::WideFonts),
            Extras::wide_fonts().packages
        );
    }
}
