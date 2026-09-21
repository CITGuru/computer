use computer_api::Action;
use computer_types::Button;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

pub const HOLD: Duration = Duration::from_secs(10);
pub const LONGEST_HOLD: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pressed {
    Button(Button),
    Key(String),
}

impl Pressed {
    pub fn key(named: &str) -> Self {
        Self::Key(computer::servers::x11::keysym(named.trim()))
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

#[derive(Default)]
pub struct Presses {
    open: Mutex<Vec<Press>>,
    next: AtomicU64,
}

struct Press {
    box_id: String,
    screen: u32,
    pressed: Pressed,
    turn: u64,
}

impl Presses {
    pub fn open(&self, box_id: &str, screen: u32, pressed: &Pressed) -> u64 {
        let turn = self.next.fetch_add(1, Ordering::Relaxed);

        if let Ok(mut open) = self.open.lock() {
            open.retain(|press| !press.is(box_id, screen, pressed));
            open.push(Press {
                box_id: box_id.to_string(),
                screen,
                pressed: pressed.clone(),
                turn,
            });
        }

        turn
    }

    pub fn close(&self, box_id: &str, screen: u32, pressed: &Pressed) -> bool {
        self.drop_where(|press| press.is(box_id, screen, pressed)) > 0
    }

    pub fn close_turn(&self, box_id: &str, screen: u32, pressed: &Pressed, turn: u64) -> bool {
        self.drop_where(|press| press.is(box_id, screen, pressed) && press.turn == turn) > 0
    }

    pub fn take_screen(&self, box_id: &str, screen: u32) -> Vec<Pressed> {
        self.take_where(|press| press.box_id == box_id && press.screen == screen)
            .into_iter()
            .map(|(_, pressed)| pressed)
            .collect()
    }

    pub fn take_box(&self, box_id: &str) -> Vec<(u32, Pressed)> {
        self.take_where(|press| press.box_id == box_id)
    }

    fn drop_where(&self, gone: impl Fn(&Press) -> bool) -> usize {
        self.take_where(gone).len()
    }

    fn take_where(&self, gone: impl Fn(&Press) -> bool) -> Vec<(u32, Pressed)> {
        let Ok(mut open) = self.open.lock() else {
            return Vec::new();
        };

        let taken = open
            .iter()
            .filter(|press| gone(press))
            .map(|press| (press.screen, press.pressed.clone()))
            .collect();
        open.retain(|press| !gone(press));
        taken
    }
}

impl Press {
    fn is(&self, box_id: &str, screen: u32, pressed: &Pressed) -> bool {
        self.box_id == box_id && self.screen == screen && &self.pressed == pressed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_release_closes_the_press_so_the_deadline_finds_nothing() {
        let presses = Presses::default();
        let left = Pressed::Button(Button::Left);
        let turn = presses.open("box-1", 0, &left);

        assert!(presses.close("box-1", 0, &left));
        assert!(
            !presses.close_turn("box-1", 0, &left, turn),
            "a button let go by hand must not be let go again when its time runs out"
        );
    }

    #[test]
    fn test_a_second_press_outlives_the_deadline_of_the_first() {
        let presses = Presses::default();
        let left = Pressed::Button(Button::Left);
        let first = presses.open("box-1", 0, &left);
        let second = presses.open("box-1", 0, &left);

        assert!(
            !presses.close_turn("box-1", 0, &left, first),
            "the first deadline would cut the second press short"
        );
        assert!(presses.close_turn("box-1", 0, &left, second));
    }

    #[test]
    fn test_a_takeover_takes_the_presses_of_its_screen_only() {
        let presses = Presses::default();
        let (left, right) = (
            Pressed::Button(Button::Left),
            Pressed::Button(Button::Right),
        );
        presses.open("box-1", 0, &left);
        presses.open("box-1", 0, &right);
        presses.open("box-1", 1, &left);
        presses.open("box-2", 0, &left);

        assert_eq!(
            presses.take_screen("box-1", 0),
            [left.clone(), right.clone()]
        );
        assert!(presses.take_screen("box-1", 0).is_empty());

        assert_eq!(presses.take_box("box-1"), [(1, left.clone())]);
        assert_eq!(presses.take_box("box-2"), [(0, left)]);
    }

    #[test]
    fn test_a_key_is_one_key_under_each_of_its_names() {
        let presses = Presses::default();
        presses.open("box-1", 0, &Pressed::key("Control"));

        assert!(
            presses.close("box-1", 0, &Pressed::key(" ctrl ")),
            "a key_up that spells the key another way must still close the press"
        );
        assert_ne!(
            Pressed::key("a"),
            Pressed::key("A"),
            "a capital is the key with shift, which is another press"
        );
    }

    #[test]
    fn test_a_takeover_takes_the_keys_with_the_buttons() {
        let presses = Presses::default();
        presses.open("box-1", 0, &Pressed::Button(Button::Left));
        presses.open("box-1", 0, &Pressed::key("shift"));

        assert_eq!(
            presses.take_screen("box-1", 0),
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
