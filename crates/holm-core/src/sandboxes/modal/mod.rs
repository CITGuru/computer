pub mod wire;

#[cfg(feature = "modal")]
pub mod cloud;
#[cfg(feature = "modal")]
pub mod proto;

pub const SERVER_URL: &str = "https://api.modal.com:443";

pub const APP_NAME: &str = "computer";
