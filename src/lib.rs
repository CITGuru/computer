//! A computer in a box.
//!
//! The API lives in `computer-core` and is re-exported whole from here; the
//! `computer` and `computerd` commands are built from here too.
//!
//! ```no_run
//! use computer::{Button, Computer, Point};
//!
//! # async fn run() -> computer::Result<()> {
//! let box_ = Computer::launch().await?;
//!
//! println!("watch it at {}", box_.viewer_url().unwrap_or_default());
//! box_.open_url("https://example.com").await?;
//!
//! let png = box_.screenshot().await?;
//! box_.click(Point::new(640, 400), Button::Left).await?;
//! box_.type_text("hello from rust").await?;
//!
//! box_.shutdown().await?;
//! # Ok(()) }
//! ```

pub use computer_core::*;
