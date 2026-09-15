//! ```no_run
//! # extern crate computer_core as computer;
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

pub mod api;
pub mod machine;
pub mod profile;

pub use api::{DEFAULT_TTL, NAME_KEY, RemoteApi, Sandbox, SandboxPlan};
pub use machine::RemoteMachine;
pub use profile::{Remote, RemoteProfile};

use crate::profile::Profile;
use std::sync::Arc;

/// The pair shares the cell the sandbox lands in, so a profile built before
/// the box exists can format a URL with the ID assigned later.
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
