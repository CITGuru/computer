use computer_types::Button;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

pub const HOLD: Duration = Duration::from_secs(10);
pub const LONGEST_HOLD: Duration = Duration::from_secs(60);

#[derive(Default)]
pub struct Presses {
    open: Mutex<Vec<Press>>,
    next: AtomicU64,
}

struct Press {
    box_id: String,
    screen: u32,
    button: Button,
    turn: u64,
}

impl Presses {
    pub fn open(&self, box_id: &str, screen: u32, button: Button) -> u64 {
        let turn = self.next.fetch_add(1, Ordering::Relaxed);

        if let Ok(mut open) = self.open.lock() {
            open.retain(|press| !press.is(box_id, screen, button));
            open.push(Press {
                box_id: box_id.to_string(),
                screen,
                button,
                turn,
            });
        }

        turn
    }

    pub fn close(&self, box_id: &str, screen: u32, button: Button) -> bool {
        self.drop_where(|press| press.is(box_id, screen, button)) > 0
    }

    pub fn close_turn(&self, box_id: &str, screen: u32, button: Button, turn: u64) -> bool {
        self.drop_where(|press| press.is(box_id, screen, button) && press.turn == turn) > 0
    }

    pub fn take_screen(&self, box_id: &str, screen: u32) -> Vec<Button> {
        self.take_where(|press| press.box_id == box_id && press.screen == screen)
            .into_iter()
            .map(|(_, button)| button)
            .collect()
    }

    pub fn take_box(&self, box_id: &str) -> Vec<(u32, Button)> {
        self.take_where(|press| press.box_id == box_id)
    }

    fn drop_where(&self, gone: impl Fn(&Press) -> bool) -> usize {
        self.take_where(gone).len()
    }

    fn take_where(&self, gone: impl Fn(&Press) -> bool) -> Vec<(u32, Button)> {
        let Ok(mut open) = self.open.lock() else {
            return Vec::new();
        };

        let taken = open
            .iter()
            .filter(|press| gone(press))
            .map(|press| (press.screen, press.button))
            .collect();
        open.retain(|press| !gone(press));
        taken
    }
}

impl Press {
    fn is(&self, box_id: &str, screen: u32, button: Button) -> bool {
        self.box_id == box_id && self.screen == screen && self.button == button
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_release_closes_the_press_so_the_deadline_finds_nothing() {
        let presses = Presses::default();
        let turn = presses.open("box-1", 0, Button::Left);

        assert!(presses.close("box-1", 0, Button::Left));
        assert!(
            !presses.close_turn("box-1", 0, Button::Left, turn),
            "a button let go by hand must not be let go again when its time runs out"
        );
    }

    #[test]
    fn test_a_second_press_outlives_the_deadline_of_the_first() {
        let presses = Presses::default();
        let first = presses.open("box-1", 0, Button::Left);
        let second = presses.open("box-1", 0, Button::Left);

        assert!(
            !presses.close_turn("box-1", 0, Button::Left, first),
            "the first deadline would cut the second press short"
        );
        assert!(presses.close_turn("box-1", 0, Button::Left, second));
    }

    #[test]
    fn test_a_takeover_takes_the_presses_of_its_screen_only() {
        let presses = Presses::default();
        presses.open("box-1", 0, Button::Left);
        presses.open("box-1", 0, Button::Right);
        presses.open("box-1", 1, Button::Left);
        presses.open("box-2", 0, Button::Left);

        assert_eq!(
            presses.take_screen("box-1", 0),
            [Button::Left, Button::Right]
        );
        assert!(presses.take_screen("box-1", 0).is_empty());

        assert_eq!(presses.take_box("box-1"), [(1, Button::Left)]);
        assert_eq!(presses.take_box("box-2"), [(0, Button::Left)]);
    }
}
