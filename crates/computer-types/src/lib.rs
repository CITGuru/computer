//! What a desktop is, and how a caller addresses one.
//!
//! Two halves. A [`Spec`] describes a desktop and says nothing about where it
//! runs, which is what lets one travel between a container, a microVM and
//! somebody else's cloud; [`Placement`] carries the other half. The rest are
//! the values every caller names — a point, a button, a selection — which a
//! server, a client and the engine all need and none of them should redefine.
//!
//! Nothing here knows about an image, and nothing here may depend on
//! `computer`: that edge runs the other way once a builder can take a spec. A spec that names no size is portable
//! across images whose natural sizes differ, and whatever compiles it applies
//! its own defaults and its own limits — an X11 image allows eight screens
//! and a macOS guest allows one, and neither number belongs in the
//! description.
//!
//! `deny_unknown_fields` throughout: a spec is written by hand, and a
//! misspelled key that is quietly ignored hands back a box missing the thing
//! it was misspelled for, with nothing anywhere saying so.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    #[serde(default)]
    pub desktop: Desktop,
    /// Named applications, which need a catalog behind them to install.
    #[serde(default)]
    pub apps: BTreeMap<String, App>,
    #[serde(default)]
    pub policy: Policy,
}

impl Spec {
    /// Two callers asking for the same desktop get the same digest, whatever
    /// order they wrote the keys in.
    ///
    /// Through [`serde_json::Value`], whose maps are ordered, so the digest
    /// follows the spec rather than the formatting it arrived in.
    pub fn digest(&self) -> String {
        let canonical = serde_json::to_value(self)
            .and_then(|value| serde_json::to_string(&value))
            .unwrap_or_default();

        let mut hasher = Sha256::new();
        hasher.update(canonical.as_bytes());
        format!("{:x}", hasher.finalize())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Desktop {
    #[serde(default)]
    pub server: DisplayServer,
    /// One size for every screen. Per-screen geometry would be a promise no
    /// image here can keep.
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    /// Screen 0 always starts, so `None` is one screen.
    #[serde(default)]
    pub screens: Option<u32>,
    #[serde(default)]
    pub features: Vec<Feature>,
    #[serde(default)]
    pub packages: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayServer {
    #[default]
    X11,
    Wayland,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    /// Chinese, Japanese, Korean and emoji. Without it those pages render as
    /// empty boxes and the screenshot still looks like a working page.
    WideFonts,
    Audio,
    Video,
    Dock,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct App {
    #[serde(default)]
    pub packages: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    #[serde(default = "yes")]
    pub network: bool,
    #[serde(default)]
    pub auth: Auth,
    #[serde(default)]
    pub bind: Bind,
    /// The host to put in a viewer URL, where it is not the one bound to.
    #[serde(default)]
    pub advertise: Option<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            network: yes(),
            auth: Auth::default(),
            bind: Bind::default(),
            advertise: None,
        }
    }
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Auth {
    #[default]
    None,
    Password,
    Token,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bind {
    #[default]
    Loopback,
    Any,
}

/// Where the box runs and for how long.
///
/// Deliberately not part of [`Spec`]: two identical desktops that differ only
/// in a memory limit are one desktop, and hashing the placement in would build
/// the same image twice.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    /// `docker`, `podman` or `nerdctl`.
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub memory: Option<String>,
    #[serde(default)]
    pub cpus: Option<String>,
    #[serde(default)]
    pub expires_after_secs: Option<u64>,
    #[serde(default)]
    pub idle_timeout_secs: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_digest_follows_the_spec_not_the_formatting() {
        let one: Spec = serde_json::from_str(r#"{"desktop":{"width":800,"height":600}}"#).unwrap();
        let two: Spec = serde_json::from_str(r#"{"desktop":{"height":600,"width":800}}"#).unwrap();

        assert_eq!(one.digest(), two.digest());
    }

    #[test]
    fn test_a_different_desktop_is_a_different_digest() {
        let one = Spec::default();
        let two = Spec {
            desktop: Desktop {
                width: Some(1920),
                ..Desktop::default()
            },
            ..Spec::default()
        };

        assert_ne!(one.digest(), two.digest());
    }

    #[test]
    fn test_naming_a_size_is_not_the_same_spec_as_leaving_it_open() {
        // One pins the geometry and travels to an image that can hold it; the
        // other takes whatever the image gives. Different desktops.
        let open = Spec::default();
        let pinned = Spec {
            desktop: Desktop {
                width: Some(1280),
                height: Some(800),
                ..Desktop::default()
            },
            ..Spec::default()
        };

        assert_ne!(open.digest(), pinned.digest());
    }

    #[test]
    fn test_a_misspelled_key_is_refused_rather_than_ignored() {
        assert!(serde_json::from_str::<Spec>(r#"{"desktop":{"widht":800}}"#).is_err());
    }

    #[test]
    fn test_a_spec_that_says_nothing_is_a_spec() {
        let spec: Spec = serde_json::from_str("{}").unwrap();

        assert!(
            spec.policy.network,
            "a box reaches the network unless told not to"
        );
        assert_eq!(spec.desktop.screens, None);
        assert!(spec.apps.is_empty());
    }
}

/// Top-left origin, device pixels, and the same coordinates the frame came
/// back in — a click against a scaled screenshot lands somewhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    pub x: u32,
    pub y: u32,
}

impl Point {
    pub const fn new(x: u32, y: u32) -> Self {
        Self { x, y }
    }
}

impl From<(u32, u32)> for Point {
    fn from((x, y): (u32, u32)) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    #[default]
    Left,
    Right,
    Middle,
}

/// Which selection is meant.
///
/// Copy and paste uses the clipboard. Dragging the mouse over text fills the
/// primary selection, which a middle click pastes. They hold different text,
/// and reading one when you meant the other returns text that looks correct
/// and is not.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Selection {
    #[default]
    Clipboard,
    Primary,
}
