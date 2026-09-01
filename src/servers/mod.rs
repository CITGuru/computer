//! The display servers a box can be driven through.
//!
//! Each is a [`Desktop`](crate::Desktop) that sends the input and captures the
//! frames, a [`DesktopFactory`](crate::DesktopFactory) that opens one per
//! screen, and a [`Profile`](crate::Profile) naming the image it drives.
//!
//! [`x11`] is the default. [`wayland`] runs the same box on sway headless,
//! where synthetic input is a compositor privilege rather than anything a
//! client may do.

#[cfg(feature = "mac")]
pub mod macos;
pub mod wayland;
pub mod x11;
