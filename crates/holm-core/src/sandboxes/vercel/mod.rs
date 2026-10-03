pub mod build;
pub mod wire;

#[cfg(feature = "vercel")]
pub mod cloud;
#[cfg(feature = "vercel")]
pub mod oidc;

pub const API_URL: &str = "https://api.vercel.com";

pub const MOST_PORTS: usize = 14;

pub const MIB_PER_CPU: u64 = 2048;

pub const MOST_TAGS: usize = 5;

pub const MOST_TAG_KEY: usize = 128;

pub const MOST_TAG_VALUE: usize = 256;

pub const LABELS_PATH: &str = "/tmp/holm-labels.json";
