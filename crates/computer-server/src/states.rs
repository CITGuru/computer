use crate::AppState;
use crate::error::ApiError;
use crate::secrets::Whose;

const KIND: &str = "states";
const SEALED: &str = "sealed:";

pub struct States;

impl States {
    pub fn valid(name: &str) -> Result<(), String> {
        let plain = name
            .chars()
            .all(|one| one.is_ascii_alphanumeric() || matches!(one, '-' | '_' | '.'));

        match !name.is_empty() && name.len() <= 40 && plain {
            true => Ok(()),
            false => Err(format!(
                "{name:?} is not a state name: letters, digits, - _ and ., 40 at most"
            )),
        }
    }
}

fn whose(key: &str) -> Whose {
    Whose::new("states", "computerd", key)
}

pub async fn keep(state: &AppState, key: &str, json: String) -> Result<(), ApiError> {
    let value = match state.secrets.holds_a_key() {
        true => {
            let secret = computer::Secret::new(json)?;
            let sealed = state
                .secrets
                .seal(&whose(key), &secret)
                .map_err(ApiError::internal)?;
            format!("{SEALED}{}", sealed.as_str())
        }
        false => json,
    };

    Ok(state.store.put_note(KIND, key, &value, None).await?)
}

pub async fn get(state: &AppState, key: &str) -> Result<Option<String>, ApiError> {
    let Some(held) = state.store.get_note(KIND, key).await? else {
        return Ok(None);
    };

    match held.strip_prefix(SEALED) {
        Some(sealed) => state
            .secrets
            .open(&whose(key), &computer_storage::Sealed::of(sealed))
            .map(|opened| Some(opened.expose().to_string()))
            .map_err(ApiError::internal),
        None => Ok(Some(held)),
    }
}

pub async fn keys(state: &AppState, prefix: &str) -> Result<Vec<String>, ApiError> {
    Ok(state
        .store
        .list_notes(KIND, prefix)
        .await?
        .into_iter()
        .map(|(key, _)| key)
        .collect())
}

pub async fn forget(state: &AppState, key: &str) -> Result<bool, ApiError> {
    let held = state.store.get_note(KIND, key).await?.is_some();
    state.store.forget_note(KIND, key).await?;

    Ok(held)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::Keeper;

    const SESSION: &str = r#"{"cookies":[{"name":"sid","value":"a-session-cookie"}]}"#;

    #[tokio::test]
    async fn test_a_state_is_kept_in_the_store_until_it_is_forgotten() {
        let state = AppState::default();
        keep(&state, "ws_a/work", SESSION.to_string())
            .await
            .expect("kept");
        keep(&state, "ws_a/home", SESSION.to_string())
            .await
            .expect("kept");
        keep(&state, "ws_b/work", SESSION.to_string())
            .await
            .expect("kept");

        assert_eq!(
            get(&state, "ws_a/work").await.expect("read").as_deref(),
            Some(SESSION)
        );
        assert_eq!(
            keys(&state, "ws_a/").await.expect("listed"),
            ["ws_a/home", "ws_a/work"]
        );
        assert!(forget(&state, "ws_a/work").await.expect("forgotten"));
        assert!(
            !forget(&state, "ws_a/work").await.expect("asked"),
            "a second forget finds nothing"
        );
        assert_eq!(get(&state, "ws_a/work").await.expect("read"), None);
    }

    #[tokio::test]
    async fn test_a_state_is_sealed_when_the_server_holds_a_key() {
        let state = AppState::default().keeping(Keeper::of(&[7u8; 32]).expect("a key"));
        keep(&state, "ws_a/work", SESSION.to_string())
            .await
            .expect("kept");

        let stored = state
            .store
            .get_note(KIND, "ws_a/work")
            .await
            .expect("read")
            .expect("a note");
        assert!(
            !stored.contains("a-session-cookie"),
            "the cookies are not readable in the store"
        );
        assert_eq!(
            get(&state, "ws_a/work").await.expect("read").as_deref(),
            Some(SESSION)
        );

        let other = AppState::split(
            std::sync::Arc::clone(&state.store),
            std::sync::Arc::clone(&state.frames),
        )
        .keeping(Keeper::of(&[8u8; 32]).expect("a key"));
        assert!(
            get(&other, "ws_a/work").await.is_err(),
            "another key does not open it"
        );
    }

    #[test]
    fn test_a_state_name_is_one_word() {
        assert!(States::valid("work").is_ok());
        assert!(States::valid("client-a.v2").is_ok());
        for wrong in ["", "two words", "a/b", &"x".repeat(41)] {
            assert!(States::valid(wrong).is_err(), "{wrong:?}");
        }
    }
}
