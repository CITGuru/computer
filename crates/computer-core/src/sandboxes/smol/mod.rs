pub mod wire;

#[cfg(feature = "smol")]
pub mod cloud;

pub const API_URL: &str = "https://api.smolmachines.com";

pub const REGISTRY: &str = "registry.smolmachines.com";

pub const MOST_PORTS: usize = 4;
