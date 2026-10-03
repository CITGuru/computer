use holm_api::Action;
use holm_storage::Store;
use holm_types::Button;
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const HOLD: Duration = Duration::from_secs(10);
pub const LONGEST_HOLD: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pressed {
    Button(Button),
    Key(String),
}

impl Pressed {
    pub fn key(named: &str) -> Self {
        Self::Key(holm::servers::x11::keysym(named.trim()))
    }

    pub fn still_down(&self) -> holm::StillDown {
        match self {
            Self::Button(button) => holm::StillDown::Button(*button),
            Self::Key(key) => holm::StillDown::Key(key.clone()),
        }
    }

    fn name(&self) -> String {
        match self {
            Self::Button(button) => format!("b:{button:?}"),
            Self::Key(key) => format!("k:{key}"),
        }
    }

    pub fn release(&self) -> Action {
        match self {
            Self::Button(button) => Action::MouseUp {
                at: None,
                button: *button,
            },
            Self::Key(key) => Action::KeyUp { key: key.clone() },
        }
    }
}

const KIND: &str = "holds";
const SLACK_MS: u64 = 5_000;

#[derive(Serialize, Deserialize)]
struct Open {
    turn: String,
    screen: u32,
    pressed: Pressed,
}

fn at(box_id: &str, screen: u32, pressed: &Pressed) -> String {
    format!("{box_id}/{screen}/{}", pressed.name())
}

fn turn() -> String {
    let mut bytes = [0u8; 8];
    let _ = getrandom::fill(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub async fn open(
    store: &dyn Store,
    box_id: &str,
    screen: u32,
    pressed: &Pressed,
    until_ms: u64,
) -> String {
    let open = Open {
        turn: turn(),
        screen,
        pressed: pressed.clone(),
    };
    let value = serde_json::to_string(&open).unwrap_or_default();

    if let Err(why) = store
        .put_note(
            KIND,
            &at(box_id, screen, pressed),
            &value,
            Some(until_ms + SLACK_MS),
        )
        .await
    {
        tracing::warn!(box_ = %box_id, %why, "a held press was not recorded");
    }
    open.turn
}

async fn held(store: &dyn Store, key: &str) -> Option<Open> {
    let value = store.get_note(KIND, key).await.ok()??;
    serde_json::from_str(&value).ok()
}

pub async fn close(store: &dyn Store, box_id: &str, screen: u32, pressed: &Pressed) -> bool {
    let key = at(box_id, screen, pressed);
    let was = held(store, &key).await.is_some();
    if was {
        let _ = store.forget_note(KIND, &key).await;
    }
    was
}

pub async fn close_turn(
    store: &dyn Store,
    box_id: &str,
    screen: u32,
    pressed: &Pressed,
    turn: &str,
) -> bool {
    let key = at(box_id, screen, pressed);
    let mine = held(store, &key)
        .await
        .is_some_and(|open| open.turn == turn);
    if mine {
        let _ = store.forget_note(KIND, &key).await;
    }
    mine
}

pub async fn held_keys(store: &dyn Store, box_id: &str, screen: u32) -> Vec<String> {
    store
        .list_notes(KIND, &format!("{box_id}/{screen}/"))
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(_, value)| serde_json::from_str::<Open>(&value).ok())
        .filter_map(|open| match open.pressed {
            Pressed::Key(key) => Some(key),
            Pressed::Button(_) => None,
        })
        .collect()
}

pub async fn take_screen(store: &dyn Store, box_id: &str, screen: u32) -> Vec<Pressed> {
    take(store, &format!("{box_id}/{screen}/"))
        .await
        .into_iter()
        .map(|(_, pressed)| pressed)
        .collect()
}

pub async fn take_box(store: &dyn Store, box_id: &str) -> Vec<(u32, Pressed)> {
    take(store, &format!("{box_id}/")).await
}

async fn take(store: &dyn Store, prefix: &str) -> Vec<(u32, Pressed)> {
    let found = store.list_notes(KIND, prefix).await.unwrap_or_default();
    let _ = store.forget_notes(KIND, prefix).await;

    found
        .into_iter()
        .filter_map(|(_, value)| serde_json::from_str::<Open>(&value).ok())
        .map(|open| (open.screen, open.pressed))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use holm_storage::memory::Memory;

    const LATER: u64 = u64::MAX / 2;

    #[tokio::test]
    async fn test_a_release_closes_the_press_so_the_deadline_finds_nothing() {
        let store = Memory::default();
        let left = Pressed::Button(Button::Left);
        let turn = open(&store, "box-1", 0, &left, LATER).await;

        assert!(close(&store, "box-1", 0, &left).await);
        assert!(
            !close_turn(&store, "box-1", 0, &left, &turn).await,
            "a button let go by hand must not be let go again when its time runs out"
        );
    }

    #[tokio::test]
    async fn test_a_second_press_outlives_the_deadline_of_the_first() {
        let store = Memory::default();
        let left = Pressed::Button(Button::Left);
        let first = open(&store, "box-1", 0, &left, LATER).await;
        let second = open(&store, "box-1", 0, &left, LATER).await;

        assert!(
            !close_turn(&store, "box-1", 0, &left, &first).await,
            "the first deadline would cut the second press short"
        );
        assert!(close_turn(&store, "box-1", 0, &left, &second).await);
    }

    #[tokio::test]
    async fn test_a_takeover_takes_the_presses_of_its_screen_only() {
        let store = Memory::default();
        let (left, right) = (
            Pressed::Button(Button::Left),
            Pressed::Button(Button::Right),
        );
        open(&store, "box-1", 0, &left, LATER).await;
        open(&store, "box-1", 0, &right, LATER).await;
        open(&store, "box-1", 1, &left, LATER).await;
        open(&store, "box-2", 0, &left, LATER).await;

        assert_eq!(
            take_screen(&store, "box-1", 0).await,
            [left.clone(), right.clone()]
        );
        assert!(take_screen(&store, "box-1", 0).await.is_empty());

        assert_eq!(take_box(&store, "box-1").await, [(1, left.clone())]);
        assert_eq!(take_box(&store, "box-2").await, [(0, left)]);
    }

    #[tokio::test]
    async fn test_a_key_is_one_key_under_each_of_its_names() {
        let store = Memory::default();
        open(&store, "box-1", 0, &Pressed::key("Control"), LATER).await;

        assert!(
            close(&store, "box-1", 0, &Pressed::key(" ctrl ")).await,
            "a key_up that spells the key another way must still close the press"
        );
        assert_ne!(
            Pressed::key("a"),
            Pressed::key("A"),
            "a capital is the key with shift, which is another press"
        );
    }

    #[tokio::test]
    async fn test_a_takeover_takes_the_keys_with_the_buttons() {
        let store = Memory::default();
        open(&store, "box-1", 0, &Pressed::Button(Button::Left), LATER).await;
        open(&store, "box-1", 0, &Pressed::key("shift"), LATER).await;

        assert_eq!(
            take_screen(&store, "box-1", 0).await,
            [Pressed::Button(Button::Left), Pressed::key("shift")]
        );
    }

    #[test]
    fn test_what_the_server_lets_go_is_traced_as_the_release_it_was() {
        assert_eq!(
            Pressed::key("shift").release(),
            Action::KeyUp {
                key: "shift".to_string()
            }
        );
        assert_eq!(
            Pressed::Button(Button::Right).release(),
            Action::MouseUp {
                at: None,
                button: Button::Right
            }
        );
    }
}
