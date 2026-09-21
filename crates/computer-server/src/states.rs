use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Default)]
pub struct States(Mutex<BTreeMap<String, String>>);

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

    pub fn keep(&self, name: &str, json: String) {
        if let Ok(mut states) = self.0.lock() {
            states.insert(name.to_string(), json);
        }
    }

    pub fn get(&self, name: &str) -> Option<String> {
        self.0.lock().ok()?.get(name).cloned()
    }

    pub fn names(&self) -> Vec<String> {
        self.0
            .lock()
            .map(|states| states.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub fn forget(&self, name: &str) -> bool {
        self.0
            .lock()
            .map(|mut states| states.remove(name).is_some())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_state_is_kept_by_name_until_it_is_forgotten() {
        let states = States::default();
        states.keep("work", "{}".to_string());
        states.keep("home", "[]".to_string());

        assert_eq!(states.get("work").as_deref(), Some("{}"));
        assert_eq!(states.names(), ["home", "work"]);
        assert!(states.forget("work"));
        assert!(!states.forget("work"), "a second forget finds nothing");
        assert_eq!(states.get("work"), None);
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
