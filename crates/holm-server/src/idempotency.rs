use holm_storage::Store;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime};

const KEEP: Duration = Duration::from_secs(600);
const KIND: &str = "replies";

pub type Fingerprint = [u8; 32];

pub fn fingerprint(route: &str, body: &[u8]) -> Fingerprint {
    let mut hasher = Sha256::new();
    hasher.update(route.as_bytes());
    hasher.update([0]);
    hasher.update(body);
    hasher.finalize().into()
}

pub enum Lookup {
    Fresh,
    Replay { status: u16, body: Vec<u8> },
    Reused,
}

#[derive(Serialize, Deserialize)]
struct Reply {
    print: String,
    status: u16,
    body: String,
}

fn hex(print: &Fingerprint) -> String {
    print.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub async fn lookup(
    store: &dyn Store,
    key: &str,
    fingerprint: Fingerprint,
) -> holm_storage::Result<Lookup> {
    let Some(held) = store.get_note(KIND, key).await? else {
        return Ok(Lookup::Fresh);
    };
    let Ok(reply) = serde_json::from_str::<Reply>(&held) else {
        return Ok(Lookup::Fresh);
    };

    Ok(match reply.print == hex(&fingerprint) {
        true => Lookup::Replay {
            status: reply.status,
            body: reply.body.into_bytes(),
        },
        false => Lookup::Reused,
    })
}

pub async fn put(
    store: &dyn Store,
    key: &str,
    fingerprint: Fingerprint,
    status: u16,
    body: &[u8],
) -> holm_storage::Result<()> {
    let reply = Reply {
        print: hex(&fingerprint),
        status,
        body: String::from_utf8_lossy(body).into_owned(),
    };
    let value = serde_json::to_string(&reply).unwrap_or_default();
    let until = crate::routes::ms_of(SystemTime::now() + KEEP);

    store.put_note(KIND, key, &value, Some(until)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use holm_storage::memory::Memory;

    #[tokio::test]
    async fn test_a_repeat_of_one_request_is_answered_with_its_reply() {
        let store = Memory::default();
        let print = fingerprint("POST /v1/boxes", b"{}");
        put(&store, "k", print, 201, b"first").await.expect("kept");

        match lookup(&store, "k", print).await.expect("read") {
            Lookup::Replay { status, body } => {
                assert_eq!(status, 201);
                assert_eq!(body, b"first");
            }
            _ => panic!("a repeat is a replay"),
        }
        assert!(matches!(
            lookup(&store, "other", print).await.expect("read"),
            Lookup::Fresh
        ));
    }

    #[tokio::test]
    async fn test_one_key_on_a_second_request_is_refused_rather_than_answered() {
        let store = Memory::default();
        put(&store, "k", fingerprint("POST /v1/boxes", b"{}"), 201, b"")
            .await
            .expect("kept");

        assert!(matches!(
            lookup(&store, "k", fingerprint("POST /v1/boxes", b"{\"a\":1}"))
                .await
                .expect("read"),
            Lookup::Reused
        ));
        assert!(matches!(
            lookup(
                &store,
                "k",
                fingerprint("POST /v1/boxes/b/screens/0/actions", b"{}")
            )
            .await
            .expect("read"),
            Lookup::Reused
        ));
    }

    #[tokio::test]
    async fn test_a_reply_is_kept_for_a_time_and_then_pruned() {
        let store = Memory::default();
        put(&store, "k", fingerprint("POST /v1/boxes", b"{}"), 201, b"")
            .await
            .expect("kept");

        let later = crate::routes::ms_of(SystemTime::now() + KEEP + Duration::from_secs(1));
        assert_eq!(store.prune_notes(later).await.expect("pruned"), 1);
    }
}
