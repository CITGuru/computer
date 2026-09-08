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
//!
//! [`remote`] is where the cloud vendors' own overlap went: holding what this
//! process started, pushing a deadline out, joining a name to the ID a sweep
//! needs. A vendor there implements seven calls and nothing else.

pub mod e2b;
pub mod microsandbox;
pub mod remote;
