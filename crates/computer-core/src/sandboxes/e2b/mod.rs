//! ```no_run
//! # extern crate computer_core as computer;
//! # #[cfg(feature = "e2b")]
//! # async fn run() -> computer::Result<()> {
//! use computer::Computer;
//! use computer::sandboxes::e2b;
//!
//! let (machine, profile) = e2b::cloud::pair_from_env()?;
//!
//! let computer = Computer::builder()
//!     .machine(std::sync::Arc::new(machine))
//!     .profile(profile)
//!     .image("your-template-id")
//!     .launch()
//!     .await?;
//!
//! let frame = computer.screenshot().await?;
//! # let _ = frame;
//! # computer.shutdown().await }
//! ```

pub mod api;
pub mod remote;
pub mod wire;

#[cfg(feature = "e2b")]
pub mod cloud;

pub use api::{E2bApi, Sandbox, SandboxPlan};
pub use remote::E2bVendor;

use crate::profile::Profile;
use crate::sandboxes::remote::{RemoteMachine, RemoteProfile};
use std::sync::Arc;

/// The pair shares the cell the sandbox lands in, so a profile built before
/// the box exists can format a URL with the ID assigned later.
pub fn pair(api: Arc<dyn E2bApi>, image: Arc<dyn Profile>) -> (RemoteMachine, Arc<RemoteProfile>) {
    crate::sandboxes::remote::pair(Arc::new(E2bVendor::new(api)), image)
}
