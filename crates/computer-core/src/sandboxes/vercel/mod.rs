pub mod build;
pub mod wire;

#[cfg(feature = "vercel")]
pub mod cloud;

pub const API_URL: &str = "https://api.vercel.com";

pub const MOST_PORTS: usize = 14;
