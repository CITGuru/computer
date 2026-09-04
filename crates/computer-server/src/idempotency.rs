//! Replies kept long enough to answer a retry with them.
//!
//! An agent whose request times out retries it. On a driving API that is not
//! a nicety: a replayed click is a double click, which on a real interface
//! opens the file rather than selecting it, and a replayed create is a second
//! box nobody asked for and everybody pays for.

use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Long enough to cover a client's retry window, short enough that the map
/// does not become storage.
const KEEP: Duration = Duration::from_secs(600);

/// A ceiling on the map itself. A reply can carry a whole PNG, and a client
/// that mints a fresh key per request never asks for any of them back, so
/// expiry alone does not bound this.
const MAX_REPLIES: usize = 256;

/// What a key was first used for.
///
/// A retry repeats one request; a key that arrives on a different one is a
/// client bug, and answering it with the first request's reply hides that bug
/// behind a success. Route and body are both here because the same body means
/// different things on different routes.
pub type Fingerprint = [u8; 32];

pub fn fingerprint(route: &str, body: &[u8]) -> Fingerprint {
    let mut hasher = Sha256::new();
    hasher.update(route.as_bytes());
    hasher.update([0]);
    hasher.update(body);
    hasher.finalize().into()
}

pub enum Lookup {
    /// This key is new, or its reply has expired.
    Fresh,
    /// The same request again: answer it with what it was answered before.
    Replay { status: u16, body: Vec<u8> },
    /// The same key on a different request.
    Reused,
}

pub struct Replies {
    entries: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    by_key: HashMap<String, Reply>,
    order: VecDeque<String>,
}

#[derive(Clone)]
struct Reply {
    at: Instant,
    fingerprint: Fingerprint,
    status: u16,
    body: Vec<u8>,
}

impl Default for Replies {
    fn default() -> Self {
        Self {
            entries: Mutex::new(Inner::default()),
        }
    }
}

impl Inner {
    fn evict(&mut self) {
        self.by_key.retain(|_, reply| reply.at.elapsed() < KEEP);
        self.order.retain(|key| self.by_key.contains_key(key));

        while self.order.len() > MAX_REPLIES {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.by_key.remove(&oldest);
        }
    }
}

impl Replies {
    pub fn lookup(&self, key: &str, fingerprint: Fingerprint) -> Lookup {
        let Ok(mut entries) = self.entries.lock() else {
            return Lookup::Fresh;
        };
        entries.evict();

        match entries.by_key.get(key) {
            None => Lookup::Fresh,
            Some(reply) if reply.fingerprint == fingerprint => Lookup::Replay {
                status: reply.status,
                body: reply.body.clone(),
            },
            Some(_) => Lookup::Reused,
        }
    }

    pub fn put(&self, key: &str, fingerprint: Fingerprint, status: u16, body: Vec<u8>) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };

        if entries
            .by_key
            .insert(
                key.to_string(),
                Reply {
                    at: Instant::now(),
                    fingerprint,
                    status,
                    body,
                },
            )
            .is_none()
        {
            entries.order.push_back(key.to_string());
        }

        entries.evict();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_repeat_of_one_request_is_answered_with_its_reply() {
        let replies = Replies::default();
        let print = fingerprint("POST /v1/boxes", b"{}");
        replies.put("k", print, 201, b"first".to_vec());

        match replies.lookup("k", print) {
            Lookup::Replay { status, body } => {
                assert_eq!(status, 201);
                assert_eq!(body, b"first");
            }
            _ => panic!("a repeat is a replay"),
        }
    }

    #[test]
    fn test_one_key_on_a_second_request_is_refused_rather_than_answered() {
        let replies = Replies::default();
        replies.put("k", fingerprint("POST /v1/boxes", b"{}"), 201, vec![]);

        assert!(matches!(
            replies.lookup("k", fingerprint("POST /v1/boxes", b"{\"a\":1}")),
            Lookup::Reused
        ));
    }

    #[test]
    fn test_one_key_on_a_second_route_is_refused() {
        let replies = Replies::default();
        replies.put("k", fingerprint("POST /v1/boxes", b"{}"), 201, vec![]);

        assert!(matches!(
            replies.lookup(
                "k",
                fingerprint("POST /v1/boxes/b/screens/0/actions", b"{}")
            ),
            Lookup::Reused
        ));
    }

    #[test]
    fn test_the_map_is_bounded_by_something_other_than_expiry() {
        let replies = Replies::default();
        let print = fingerprint("POST /v1/boxes", b"{}");

        for n in 0..MAX_REPLIES + 50 {
            replies.put(&format!("k{n}"), print, 200, vec![0; 16]);
        }

        let Ok(entries) = replies.entries.lock() else {
            panic!("the map");
        };
        assert!(entries.by_key.len() <= MAX_REPLIES);
        assert_eq!(entries.by_key.len(), entries.order.len());
    }

    #[test]
    fn test_the_oldest_key_is_the_one_dropped() {
        let replies = Replies::default();
        let print = fingerprint("POST /v1/boxes", b"{}");

        for n in 0..MAX_REPLIES + 1 {
            replies.put(&format!("k{n}"), print, 200, vec![]);
        }

        assert!(matches!(replies.lookup("k0", print), Lookup::Fresh));
        assert!(matches!(
            replies.lookup(&format!("k{MAX_REPLIES}"), print),
            Lookup::Replay { .. }
        ));
    }
}
