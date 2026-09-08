//! A box in somebody else's cloud, whoever that is.
//!
//! [`DockerMachine`](crate::DockerMachine) needs a container runtime here and
//! [`MicroVm`](crate::MicroVm) needs a hypervisor here. Both put the desktop
//! where the program runs. A sandbox vendor does not, so a service on a small
//! host can hand out desktops with no `/dev/kvm` and no capacity planning of
//! its own, and the boundary is still a kernel the box does not share.
//!
//! Every such vendor answers the same shape: create a sandbox, run a command
//! in it, move a file, kill it, and publish its ports at an address of the
//! vendor's own. [`RemoteApi`] is that shape. Implement it and
//! [`RemoteMachine`] gives you a [`Machine`](crate::Machine) — with the parts
//! that are nobody's vendor-specific business already written: what this
//! process started, the lazy deadline, the name-to-ID join a sweep needs.
//!
//! [`e2b`](super::e2b) is the worked example: everything E2B does that nobody
//! else does — a port that is a subdomain, two tokens where this carries one,
//! an image that is a template — is one short file beside its HTTP client.
//!
//! # Seven methods
//!
//! [`RemoteApi::vendor`], [`available`](RemoteApi::available),
//! [`create`](RemoteApi::create), [`find`](RemoteApi::find),
//! [`kill`](RemoteApi::kill), [`exec`](RemoteApi::exec),
//! [`read`](RemoteApi::read) and [`write`](RemoteApi::write). The rest have
//! defaults, and each default is the honest answer for a vendor that lacks the
//! thing: no deadline to push out, no listing to sweep, no way to kill a
//! sandbox from a signal handler.
//!
//! # What changes when the box leaves this host
//!
//! - **Driving is identical.** A screen command is a command, and a sandbox
//!   runs one.
//! - **A port is an address the vendor chose**, so [`RemoteProfile`] rewrites
//!   the viewer URL from [`Sandbox::endpoints`] and the machine reports an
//!   identity port map.
//! - **DevTools does not reach.** An endpoint out here is `wss` on a public
//!   host and [`crate::cdp`] speaks plain TCP, so the profile withdraws the
//!   claim rather than publishing a port to nowhere.
//! - **The image is the vendor's.** This crate builds container images; a
//!   vendor runs a template or a snapshot it built itself, and
//!   [`RemoteApi::ensure_image`] refuses the mismatch with the way across.
//! - **The viewer URL is withheld** unless you ask for it. See
//!   [`RemoteMachine::public_viewer`], which explains why that is not privacy.
//!
//! # Writing one
//!
//! ```no_run
//! use computer::sandboxes::remote::{self, RemoteApi, Sandbox, SandboxPlan};
//! use computer::{Computer, Result, X11Profile};
//! use std::sync::Arc;
//!
//! # struct Daytona;
//! # #[async_trait::async_trait]
//! # impl RemoteApi for Daytona {
//! #     fn vendor(&self) -> &str { "daytona" }
//! #     async fn available(&self) -> Result<()> { Ok(()) }
//! #     async fn create(&self, _plan: &SandboxPlan) -> Result<Sandbox> { todo!() }
//! #     async fn find(&self, _name: &str) -> Result<Option<Sandbox>> { todo!() }
//! #     async fn kill(&self, _id: &str) -> Result<()> { todo!() }
//! #     async fn exec(&self, _s: &Sandbox, _argv: &[String],
//! #         _env: &std::collections::BTreeMap<String, String>)
//! #         -> Result<computer::ExecResult> { todo!() }
//! #     async fn read(&self, _s: &Sandbox, _path: &str) -> Result<Vec<u8>> { todo!() }
//! #     async fn write(&self, _s: &Sandbox, _path: &str, _bytes: &[u8]) -> Result<()> { todo!() }
//! # }
//! # async fn run() -> Result<()> {
//! let (machine, profile) = remote::pair(Arc::new(Daytona), Arc::new(X11Profile));
//!
//! let computer = Computer::builder()
//!     .machine(Arc::new(machine))
//!     .profile(profile)
//!     .image("your-snapshot")
//!     .launch()
//!     .await?;
//!
//! let frame = computer.screenshot().await?;
//! # let _ = frame;
//! # computer.shutdown().await }
//! ```
//!
//! [`crate::testing::ScriptedRemote`] tests all of that with no account and no
//! network. `examples/custom_sandbox.rs` is a whole vendor in one file.

pub mod api;
pub mod machine;
pub mod profile;

pub use api::{DEFAULT_TTL, NAME_KEY, RemoteApi, Sandbox, SandboxPlan};
pub use machine::RemoteMachine;
pub use profile::{Remote, RemoteProfile};

use crate::profile::Profile;
use std::sync::Arc;

/// A machine and the profile that goes with it.
///
/// They share the cell the sandbox lands in, which is what lets a profile
/// built before the box exists format a URL containing an ID the vendor had
/// not assigned yet. Building them apart is possible and gets the pairing
/// wrong quietly, so this is the door.
pub fn pair(
    api: Arc<dyn RemoteApi>,
    image: Arc<dyn Profile>,
) -> (RemoteMachine, Arc<RemoteProfile>) {
    let remote = Arc::new(Remote::new());

    (
        RemoteMachine::new(api, Arc::clone(&remote)),
        Arc::new(RemoteProfile::new(image, remote)),
    )
}
