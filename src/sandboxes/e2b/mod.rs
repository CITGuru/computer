//! E2B: the box in somebody else's Firecracker rather than on this host.
//!
//! [`DockerMachine`](crate::DockerMachine) needs a container runtime here and
//! [`MicroVm`](crate::MicroVm) needs a hypervisor here. Both put the desktop
//! where the program runs. This does not, so a service on a small host can
//! hand out desktops with no `/dev/kvm` and no capacity planning of its own,
//! and the boundary is still a kernel the box does not share.
//!
//! # Why not a `MicroVmApi`
//!
//! [`Plan`](crate::microvm::Plan) carries host-to-guest port pairs, and
//! [`MicroVm::start`](crate::MicroVm) picks free host ports before it creates
//! the machine, because a hypervisor forwards the pairs it is given. E2B
//! forwards none: it publishes a hostname per port and there is no host side
//! to choose. So this goes through [`crate::sandboxes::remote`] instead, which
//! is that shape — and which
//! [`RemoteMachine`](crate::sandboxes::remote::RemoteMachine) implements once
//! for every vendor of it.
//!
//! # What is E2B's own
//!
//! [`E2bVendor`] is the whole of it, and it is short:
//!
//! - **A port is a subdomain**, `6080-<id>.e2b.app`, so the endpoints are
//!   formatted rather than handed back by the control plane.
//! - **A call needs two tokens**, envd for the data plane and a traffic token
//!   for the proxy, where the seam carries one. The rest is held by ID.
//! - **The image is a template.** E2B builds those itself; `ensure_image`
//!   refuses a container tag with the way across.
//!
//! Everything else — driving, the lazy deadline, the sweep, the withdrawn
//! DevTools claim, the viewer that is withheld until asked for — is the
//! generic half, and is written down there.
//!
//! # Reaching a real one
//!
//! [`E2bApi`] is the seam onto E2B itself, and it needs no feature: a caller
//! with their own HTTP client implements it and gets everything above.
//! [`cloud`] is the implementation that ships, behind `--features e2b`,
//! because the control plane is `https` on somebody else's host and there is
//! no command here that already knows how to reach it.
//!
//! ```no_run
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
