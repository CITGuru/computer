// `deny_unknown_fields` throughout: a misspelled key must not be silently ignored.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    #[serde(default)]
    pub desktop: Desktop,
    #[serde(default)]
    pub apps: BTreeMap<String, App>,
    #[serde(default)]
    pub policy: Policy,
}

impl Spec {
    /// Through [`serde_json::Value`], so key order does not change the digest.
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
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
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
    WideFonts,
    Audio,
    Video,
    Dock,
    X11Apps,
    /// Not for a running box: an app joins the tree only if it started after the bus.
    Accessibility,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct App {
    #[serde(default)]
    pub packages: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settle_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub key_url: String,
    /// Without the `signed-by`, which this crate fills in.
    pub list: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum WindowMatch {
    Class(String),
    Title(String),
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
    #[serde(default)]
    pub advertise: Option<String>,
    #[serde(default)]
    pub custom_sources: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            network: yes(),
            auth: Auth::default(),
            bind: Bind::default(),
            advertise: None,
            custom_sources: false,
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

/// Not part of [`Spec`], so a placement change does not rebuild the image.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
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
    fn test_a_press_takes_one_chord_or_several() {
        assert_eq!("ctrl+a".chords(), ["ctrl+a"]);
        assert_eq!("enter".to_string().chords(), ["enter"]);
        assert_eq!(["tab", "tab"].chords(), ["tab", "tab"]);
        assert_eq!(vec!["a".to_string()].chords(), ["a"]);

        let several: &[&str] = &["up", "down"];
        assert_eq!(several.chords(), ["up", "down"]);
    }

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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    /// Valid only until the tree next changes.
    pub id: String,
    pub app: String,
    pub role: String,
    pub name: String,
    #[serde(default)]
    pub actions: Vec<String>,
    #[serde(default)]
    pub states: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labelled: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Point>,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeQuery {
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default)]
    pub exact: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Selection {
    #[default]
    Clipboard,
    Primary,
}

impl Selection {
    pub fn name(self) -> &'static str {
        match self {
            Self::Clipboard => "clipboard",
            Self::Primary => "primary",
        }
    }
}

pub trait Keys {
    fn chords(self) -> Vec<String>;
}

impl Keys for &str {
    fn chords(self) -> Vec<String> {
        vec![self.to_string()]
    }
}

impl Keys for String {
    fn chords(self) -> Vec<String> {
        vec![self]
    }
}

impl Keys for Vec<String> {
    fn chords(self) -> Vec<String> {
        self
    }
}

impl<T: AsRef<str>> Keys for &[T] {
    fn chords(self) -> Vec<String> {
        self.iter().map(|one| one.as_ref().to_string()).collect()
    }
}

impl<T: AsRef<str>, const N: usize> Keys for [T; N] {
    fn chords(self) -> Vec<String> {
        self.iter().map(|one| one.as_ref().to_string()).collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Held {
    Shift,
    Ctrl,
    Alt,
    Super,
}

impl Held {
    pub fn named(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "shift" => Some(Self::Shift),
            "ctrl" | "control" => Some(Self::Ctrl),
            "alt" | "option" => Some(Self::Alt),
            "meta" | "cmd" | "command" | "super" | "win" => Some(Self::Super),
            _ => None,
        }
    }

    pub fn keysym(self) -> &'static str {
        match self {
            Self::Shift => "shift",
            Self::Ctrl => "ctrl",
            Self::Alt => "alt",
            Self::Super => "super",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rect {
    pub at: Point,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const fn new(at: Point, width: u32, height: u32) -> Self {
        Self { at, width, height }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub at: Point,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "how", rename_all = "snake_case", deny_unknown_fields)]
pub enum Arrange {
    At {
        to: Point,
    },
    Size {
        width: u32,
        height: u32,
    },
    Maximise,
    /// Sway has no such state, so there it is the scratchpad.
    Minimise,
    Restore,
}
