//! Places a box can run that are not the container runtime on this machine.
//!
//! [`crate::machine::DockerMachine`] covers `docker`, `podman` and `nerdctl`,
//! which are one implementation because they take the same arguments. A
//! sandbox vendor is not: each has its own command, its own idea of an image
//! and its own way of being asked whether a machine is up.
//!
//! So each gets a directory rather than a module tacked onto the abstraction
//! it implements. What they have in common already has a home —
//! [`crate::microvm::MicroVmApi`] for a hypervisor, [`crate::Machine`] for
//! anywhere a box can be — and nothing vendor-specific belongs in either.

pub mod e2b;
pub mod microsandbox;
