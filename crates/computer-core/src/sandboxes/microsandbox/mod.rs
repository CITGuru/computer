//! microsandbox, reached two ways.
//!
//! [`msb`] drives the command the installer puts on disk, and needs no crate
//! feature: it is arguments in and text out, which is testable without a
//! hypervisor anywhere near it. [`vendor`] links the library instead, behind
//! `--features microsandbox`, for callers who would rather not shell out.
//!
//! Both produce a [`crate::microvm::MicroVm`], so everything above them — the
//! driver, the screens, the takeover gate, the descriptor — is the same code a
//! container uses.

pub mod msb;

#[cfg(feature = "microsandbox")]
pub mod vendor;
