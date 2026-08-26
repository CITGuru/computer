//! Checking that the claims are true.
//!
//! [`crate::DesktopSupport`] is written when the descriptor is designed rather than
//! when the capability is built, so a flag can stay true beside a method that
//! answers `Unsupported`, or an image with no tool behind it. The compiler
//! cannot catch that: it guarantees the method exists, not that it works.
//!
//! [`audit`] calls each claimed capability against a running box and reports
//! the ones nothing serves. Both live tests end with one.

use crate::error::Result;
use crate::{Computer, Delta, Point};
use std::time::Duration;

/// A claim that nothing serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unmet {
    pub claim: &'static str,
    pub detail: String,
}

/// What the descriptor claimed, and what answered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Audit {
    /// Claims that were exercised and worked.
    pub met: Vec<&'static str>,
    /// Claims that were exercised and did not.
    pub unmet: Vec<Unmet>,
    /// Claims nothing here can check, and why not.
    pub skipped: Vec<Unmet>,
}

impl Audit {
    /// Whether every claim that could be checked was true.
    pub fn ok(&self) -> bool {
        self.unmet.is_empty()
    }

    fn met(&mut self, claim: &'static str) {
        self.met.push(claim);
    }

    fn failed(&mut self, claim: &'static str, detail: impl Into<String>) {
        self.unmet.push(Unmet {
            claim,
            detail: detail.into(),
        });
    }

    fn skip(&mut self, claim: &'static str, detail: impl Into<String>) {
        self.skipped.push(Unmet {
            claim,
            detail: detail.into(),
        });
    }

    fn check(&mut self, claim: &'static str, outcome: Result<()>) {
        match outcome {
            Ok(()) => self.met(claim),
            Err(error) => self.failed(claim, error.to_string()),
        }
    }
}

impl std::fmt::Display for Audit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} met", self.met.len())?;
        for gap in &self.unmet {
            write!(f, "; {} does not work: {}", gap.claim, gap.detail)?;
        }
        for skipped in &self.skipped {
            write!(f, "; {} not checked ({})", skipped.claim, skipped.detail)?;
        }
        Ok(())
    }
}

/// Exercise every capability the box claims.
///
/// The screen is left as it was found, apart from the pointer.
pub async fn audit(computer: &Computer) -> Audit {
    let mut audit = Audit::default();
    let support = computer.support().clone();
    let screen = computer.primary();

    if let Some(display) = support.display {
        // A capture, and the size it claims. A frame that comes back at another
        // size makes every coordinate in the descriptor wrong.
        let outcome = async {
            let frame = screen.screenshot().await?;
            if frame.first_chunk::<4>() != Some(&[0x89, b'P', b'N', b'G']) {
                return Err(crate::Error::denied("the capture is not a PNG"));
            }

            let (width, height) = screen.geometry().await?;
            if (width, height) != (display.width, display.height) {
                return Err(crate::Error::denied(format!(
                    "the descriptor says {}x{} and the screen is {width}x{height}",
                    display.width, display.height
                )));
            }
            Ok(())
        }
        .await;
        audit.check("display", outcome);
    }

    if support.input {
        // Moved and then measured: a driver that delivers no input still
        // answers every call successfully.
        let outcome = async {
            let at = Point::new(7, 11);
            screen.move_to(at).await?;
            let landed = screen.cursor().await?;

            if landed != at {
                return Err(crate::Error::denied(format!(
                    "the pointer was sent to {},{} and is at {},{}",
                    at.x, at.y, landed.x, landed.y
                )));
            }
            screen.scroll(at, Delta::down(1)).await
        }
        .await;
        audit.check("input", outcome);
    }

    if let Some(browser) = &support.browser {
        if browser.cdp {
            let outcome = match computer.browser() {
                None => Err(crate::Error::denied(
                    "no DevTools port reaches this box from here",
                )),
                Some(devtools) => devtools.version().await.map(|_| ()),
            };
            audit.check("browser", outcome);
        }
    }

    if support.clipboard {
        // A round trip, because a write that reports success and a read that
        // answers the previous value look identical from one side.
        let token = format!("audit-{}", std::process::id());
        let outcome = async {
            screen.set_clipboard(&token).await?;
            let back = screen.clipboard().await?;

            if back != token {
                return Err(crate::Error::denied(format!(
                    "wrote {token:?} and read back {back:?}"
                )));
            }
            Ok(())
        }
        .await;
        audit.check("clipboard", outcome);
    }

    if let Some(viewer) = &support.viewer {
        let outcome = match screen.viewer_url() {
            None => Err(crate::Error::denied(
                "no viewer port reaches this box from here",
            )),
            Some(_) => screen.viewers().await.map(|_| ()),
        };
        audit.check("viewer", outcome);

        if viewer.takeover {
            // Started and ended, so the audit leaves the screen as it found it.
            let outcome = async {
                let takeover = screen.hand_over().await?;
                let refused = screen.click(Point::new(1, 1), crate::Button::Left).await;
                let ended = takeover.end().await;

                if refused.is_ok() {
                    return Err(crate::Error::denied(
                        "the gate let the owner act while a person was driving",
                    ));
                }
                ended
            }
            .await;
            audit.check("takeover", outcome);
        }
    }

    if support.max_screens > 1 {
        // Not exercised: each screen is an X server, a window manager and a
        // browser. `tests/image.rs` checks the count against the script.
        audit.skip(
            "max_screens",
            format!(
                "{} screens would cost about {} GB to start",
                support.max_screens,
                support.max_screens / 4
            ),
        );
    }

    audit
}

/// Audit, and fail where a claim does not hold.
pub async fn audit_strictly(computer: &Computer, within: Duration) -> Result<Audit> {
    let audit = tokio::time::timeout(within, audit(computer))
        .await
        .map_err(|_| crate::Error::Timeout {
            after: within,
            detail: "the audit did not finish".to_string(),
        })?;

    if audit.ok() {
        Ok(audit)
    } else {
        Err(crate::Error::denied(audit.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_an_audit_with_nothing_unmet_is_ok() {
        let mut audit = Audit::default();
        audit.met("display");
        audit.skip("max_screens", "too expensive");

        assert!(audit.ok(), "a skipped claim is not a failed one");
        assert!(audit.to_string().contains("not checked"));
    }

    #[test]
    fn test_one_broken_claim_fails_the_audit() {
        let mut audit = Audit::default();
        audit.met("display");
        audit.failed("clipboard", "wrote \"a\" and read back \"\"");

        assert!(!audit.ok());
        assert!(audit.to_string().contains("clipboard does not work"));
    }

    #[test]
    fn test_a_claim_that_errored_is_reported_with_the_reason() {
        let mut audit = Audit::default();
        audit.check("input", Err(crate::Error::denied("no such display")));

        assert_eq!(audit.unmet.len(), 1);
        assert!(audit.unmet[0].detail.contains("no such display"));
    }
}
