//! The display servers a box can be driven through.
//!
//! Each is a [`Desktop`](crate::Desktop) that sends the input and captures the
//! frames, a [`DesktopFactory`](crate::DesktopFactory) that opens one per
//! screen, and a [`Profile`](crate::Profile) naming the image it drives.
//!
//! [`x11`] is the default. [`wayland`] runs the same box on sway headless,
//! where synthetic input is a compositor privilege rather than anything a
//! client may do.

pub mod wayland;
pub mod x11;

use crate::error::{Error, Result};
use crate::exec::ExecResult;
use std::time::Duration;

/// A loop out here would pay a container exec per probe, and the whole point
/// is to be cheaper than the sleep it replaces. `capture` is whatever the
/// display server captures a frame with; only its bytes are compared, so it
/// need not be a picture a caller would ever see.
pub(crate) fn still_argv(capture: &str, settle: Duration, within: Duration) -> Vec<String> {
    let settle_ms = settle.as_millis();
    let within_ms = within.as_millis();

    vec![
        "sh".to_string(),
        "-c".to_string(),
        format!(
            r#"end=$(( $(date +%s%N) / 1000000 + {within_ms} )); last=""; since=0
while [ $(( $(date +%s%N) / 1000000 )) -lt $end ]; do
  h=$({capture} | cksum | cut -d' ' -f1)
  now=$(( $(date +%s%N) / 1000000 ))
  if [ "$h" = "$last" ] && [ -n "$h" ]; then
    [ $since -eq 0 ] && since=$now
    if [ $(( now - since )) -ge {settle_ms} ]; then
      echo still
      exit 0
    fi
  else
    last="$h"; since=0
  fi
  sleep 0.1
done
echo moving"#
        ),
    ]
}

pub(crate) fn settled(result: ExecResult, within: Duration) -> Result<()> {
    match result.stdout_utf8().trim() {
        "still" => Ok(()),
        _ => Err(Error::Timeout {
            after: within,
            detail: "the screen never held still".to_string(),
        }),
    }
}
