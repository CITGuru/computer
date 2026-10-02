use holm_storage::Store;

const KIND: &str = "labels";

pub struct Labels;

impl Labels {
    pub fn valid(label: &str) -> Result<(), String> {
        let plain = label
            .chars()
            .all(|one| one.is_ascii_alphanumeric() || one == '-' || one == '_');
        let an_id = label.len() == 32 && label.chars().all(|one| one.is_ascii_hexdigit());

        match (label.is_empty() || label.len() > 40 || !plain, an_id) {
            (true, _) => Err(format!(
                "{label:?} is not a label: letters, digits, - and _, 40 at most"
            )),
            (_, true) => Err(format!("{label} reads as a tab id, which a label must not")),
            _ => Ok(()),
        }
    }
}

fn key(box_id: &str, label: &str) -> String {
    format!("{box_id}/{label}")
}

pub async fn set(store: &dyn Store, box_id: &str, label: &str, target: &str) {
    if let Err(why) = store
        .put_note(KIND, &key(box_id, label), target, None)
        .await
    {
        tracing::warn!(box_ = %box_id, %why, "a label was not kept");
    }
}

pub async fn target(store: &dyn Store, box_id: &str, word: &str) -> Option<String> {
    store
        .get_note(KIND, &key(box_id, word))
        .await
        .ok()
        .flatten()
}

pub async fn live(store: &dyn Store, box_id: &str, live: &[String]) -> Vec<(String, String)> {
    let prefix = key(box_id, "");
    let held = store.list_notes(KIND, &prefix).await.unwrap_or_default();
    let mut kept = Vec::new();

    for (at, target) in held {
        match live.contains(&target) {
            true => kept.push((at.trim_start_matches(&prefix).to_string(), target)),
            false => {
                let _ = store.forget_note(KIND, &at).await;
            }
        }
    }

    kept
}

pub async fn forget(store: &dyn Store, box_id: &str) {
    if let Err(why) = store.forget_notes(KIND, &key(box_id, "")).await {
        tracing::warn!(box_ = %box_id, %why, "the labels of a box were not forgotten");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use holm_storage::memory::Memory;

    #[tokio::test]
    async fn test_a_label_names_a_tab_of_one_box_until_the_tab_is_gone() {
        let store = Memory::default();
        set(&store, "box-1", "docs", "AAAA").await;
        set(&store, "box-2", "docs", "BBBB").await;

        assert_eq!(
            target(&store, "box-1", "docs").await.as_deref(),
            Some("AAAA")
        );
        assert_eq!(
            target(&store, "box-2", "docs").await.as_deref(),
            Some("BBBB")
        );
        assert_eq!(
            target(&store, "box-1", "AAAA").await,
            None,
            "a word that is no label is left to be read as an id"
        );
        assert_eq!(
            live(&store, "box-1", &["AAAA".to_string()]).await,
            [("docs".to_string(), "AAAA".to_string())]
        );

        assert!(
            live(&store, "box-1", &["CCCC".to_string()])
                .await
                .is_empty()
        );
        assert_eq!(
            target(&store, "box-1", "docs").await,
            None,
            "a label that outlived its tab would send the next action to a tab that is not there"
        );
        assert_eq!(
            target(&store, "box-2", "docs").await.as_deref(),
            Some("BBBB")
        );

        forget(&store, "box-2").await;
        assert_eq!(target(&store, "box-2", "docs").await, None);
    }

    #[tokio::test]
    async fn test_a_label_moves_to_the_tab_it_was_last_given() {
        let store = Memory::default();
        set(&store, "box-1", "docs", "AAAA").await;
        set(&store, "box-1", "docs", "BBBB").await;

        assert_eq!(
            target(&store, "box-1", "docs").await.as_deref(),
            Some("BBBB")
        );
        assert!(
            live(&store, "box-1", &["AAAA".to_string()])
                .await
                .is_empty()
        );
    }

    #[test]
    fn test_a_label_cannot_be_mistaken_for_an_id_or_a_flag() {
        assert!(Labels::valid("docs").is_ok());
        assert!(Labels::valid("pr_27-review").is_ok());
        assert!(Labels::valid("").is_err());
        assert!(Labels::valid("two words").is_err());
        assert!(Labels::valid(&"x".repeat(41)).is_err());
        assert!(
            Labels::valid("5D320217F98F2C0E1D7A52A7BB4B2D92").is_err(),
            "a tab id is 32 hex digits, and a label shaped like one would shadow a real tab"
        );
    }
}
