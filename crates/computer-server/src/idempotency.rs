//! Replies kept long enough to answer a retry with them.
//!
//! An agent whose request times out retries it. On a driving API that is not
//! a nicety: a replayed click is a double click, which on a real interface
//! opens the file rather than selecting it, and a replayed create is a second
//! box nobody asked for and everybody pays for.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Long enough to cover a client's retry window, short enough that the map
/// does not become storage.
const KEEP: Duration = Duration::from_secs(600);

pub struct Replies {
    entries: Mutex<HashMap<String, Reply>>,
}

#[derive(Clone)]
struct Reply {
    at: Instant,
    status: u16,
    body: Vec<u8>,
}

impl Default for Replies {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }
}

impl Replies {
    pub fn get(&self, key: &str) -> Option<(u16, Vec<u8>)> {
        let mut entries = self.entries.lock().ok()?;
        entries.retain(|_, reply| reply.at.elapsed() < KEEP);

        entries
            .get(key)
            .map(|reply| (reply.status, reply.body.clone()))
    }

    pub fn put(&self, key: &str, status: u16, body: Vec<u8>) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };

        entries.insert(
            key.to_string(),
            Reply {
                at: Instant::now(),
                status,
                body,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_retry_gets_the_first_answer_rather_than_a_second_run() {
        let replies = Replies::default();
        replies.put("k", 201, b"first".to_vec());

        assert_eq!(replies.get("k"), Some((201, b"first".to_vec())));
    }

    #[test]
    fn test_an_unseen_key_has_nothing() {
        assert_eq!(Replies::default().get("k"), None);
    }
}
