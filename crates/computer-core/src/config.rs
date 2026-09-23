use crate::bundle;
use crate::image;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub image: String,
    pub width: u32,
    pub height: u32,
    pub network: bool,
    pub publish: Vec<u16>,
    pub bind: crate::Bind,
    pub auth: crate::Auth,
    pub credentials: Option<crate::Credentials>,
    pub advertise: Option<String>,
    pub env: BTreeMap<String, String>,
    pub memory: Option<String>,
    pub cpus: Option<String>,
    pub isolation: Option<String>,
    pub shm_size: Option<String>,
    /// A named volume survives `rm --volumes`, so boxes given the same name share a browser.
    pub profiles: Option<String>,
    pub labels: BTreeMap<String, String>,
    pub extras: bundle::Extras,
    pub bundle: Option<bundle::Bundle>,
    /// Mutually exclusive with [`Config::bundle`].
    pub image_dir: Option<PathBuf>,
    pub boot: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            image: bundle::DESKTOP.tag(),
            width: image::WIDTH,
            height: image::HEIGHT,
            network: true,
            publish: Vec::new(),
            bind: crate::Bind::Loopback,
            auth: crate::Auth::Open,
            credentials: None,
            advertise: None,
            env: BTreeMap::new(),
            memory: None,
            cpus: None,
            isolation: None,
            shm_size: None,
            profiles: None,
            labels: BTreeMap::new(),
            extras: bundle::Extras::none(),
            bundle: Some(bundle::DESKTOP),
            image_dir: None,
            boot: Vec::new(),
        }
    }
}

pub const PROFILES: &str = "/home/computer/.browser-profiles";
