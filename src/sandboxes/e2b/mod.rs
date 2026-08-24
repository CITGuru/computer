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
//! to choose. Every port field would be dead and the map would be one nothing
//! made. So this implements [`Machine`](crate::Machine) directly.
//!
//! # What moves and what does not
//!
//! - **Driving is identical.** A screen command is a command, and a sandbox
//!   runs one.
//! - **A port is a subdomain**, `6080-<id>.e2b.app`, so [`E2bProfile`] rewrites
//!   the viewer URL and the machine reports an identity port map.
//! - **DevTools does not reach.** An endpoint out here is `wss` on a public
//!   host and [`crate::cdp`] speaks plain TCP, so the profile withdraws the
//!   claim rather than publishing a port to nowhere.
//! - **The screen has no password**, and a sandbox URL is on the internet. See
//!   [`E2bMachine::public_viewer`].
//! - **The image is a template.** E2B builds those itself; `ensure_image`
//!   refuses a container tag with the way across.
//!
//! # Reaching a real one
//!
//! [`E2bApi`] is the whole seam, and it needs no feature: a caller with their
//! own HTTP client implements it and gets everything above. [`cloud`] is the
//! implementation that ships, behind `--features e2b`, because the control
//! plane is `https` on somebody else's host and there is no command here that
//! already knows how to reach it.
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
pub mod machine;
pub mod profile;
pub mod wire;

#[cfg(feature = "e2b")]
pub mod cloud;

pub use api::{E2bApi, Sandbox, SandboxPlan};
pub use machine::E2bMachine;
pub use profile::{E2bProfile, Reachable};

use crate::profile::Profile;
use std::sync::Arc;

/// A machine and the profile that goes with it.
///
/// They share the cell the sandbox lands in, which is what lets a profile
/// built before the box exists format a URL containing an ID the control plane
/// had not assigned yet. Building them apart is possible and gets the pairing
/// wrong quietly, so this is the door.
pub fn pair(api: Arc<dyn E2bApi>, image: Arc<dyn Profile>) -> (E2bMachine, Arc<E2bProfile>) {
    let reachable = Arc::new(Reachable::new());

    (
        E2bMachine::new(api, Arc::clone(&reachable)),
        Arc::new(E2bProfile::new(image, reachable)),
    )
}
