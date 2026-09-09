//! E2B: the box in somebody else's Firecracker rather than on this host.
//!
//! One vendor of [`crate::sandboxes::remote`], which is where the case for
//! putting a desktop off this host is made and where everything a cloud
//! sandbox does the same way is implemented. [`E2bVendor`] is the rest: a port
//! that is a subdomain, a second token the seam has nowhere to put, and an
//! image that is a template E2B builds.
//!
//! [`E2bApi`] is the seam onto E2B itself and needs no feature. [`cloud`] is
//! the implementation that ships, behind `--features e2b`, because the control
//! plane is `https` on somebody else's host and no command here already knows
//! how to reach it.
//!
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

/// A machine and the profile that goes with it.
///
/// They share the cell the sandbox lands in, which is what lets a profile
/// built before the box exists format a URL containing an ID the control plane
/// had not assigned yet. Building them apart is possible and gets the pairing
/// wrong quietly, so this is the door.
pub fn pair(api: Arc<dyn E2bApi>, image: Arc<dyn Profile>) -> (RemoteMachine, Arc<RemoteProfile>) {
    crate::sandboxes::remote::pair(Arc::new(E2bVendor::new(api)), image)
}
