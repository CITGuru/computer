pub mod a11y;
pub mod wayland;
pub mod x11;

use crate::Held;
use crate::error::{Error, Result};
use crate::exec::ExecResult;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Default)]
pub(crate) struct KeysDown(Mutex<Vec<String>>);

impl KeysDown {
    pub(crate) fn any(&self) -> bool {
        self.0.lock().map(|keys| !keys.is_empty()).unwrap_or(false)
    }

    pub(crate) fn set(&self, key: &str, down: bool) {
        if let Ok(mut keys) = self.0.lock() {
            keys.retain(|held| held != key);
            if down {
                keys.push(key.to_string());
            }
        }
    }

    pub(crate) fn clear(&self) {
        if let Ok(mut keys) = self.0.lock() {
            keys.clear();
        }
    }

    pub(crate) fn without(&self, held: &[Held]) -> Vec<Held> {
        let Ok(keys) = self.0.lock() else {
            return held.to_vec();
        };
        held.iter()
            .copied()
            .filter(|one| !keys.iter().any(|key| key == one.keysym()))
            .collect()
    }
}

/// Runs in the box because a loop out here pays a container exec per probe.
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
