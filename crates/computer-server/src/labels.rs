use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub struct Labels(Mutex<HashMap<(String, String), String>>);

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

    pub fn set(&self, box_id: &str, label: &str, target: &str) {
        if let Ok(mut labels) = self.0.lock() {
            labels.insert((box_id.to_string(), label.to_string()), target.to_string());
        }
    }

    pub fn target(&self, box_id: &str, word: &str) -> Option<String> {
        self.0
            .lock()
            .ok()?
            .get(&(box_id.to_string(), word.to_string()))
            .cloned()
    }

    pub fn of(&self, box_id: &str, target: &str) -> Option<String> {
        let labels = self.0.lock().ok()?;
        let mut named: Vec<&String> = labels
            .iter()
            .filter(|((owner, _), held)| owner == box_id && held.as_str() == target)
            .map(|((_, label), _)| label)
            .collect();
        named.sort();
        named.first().map(|label| (*label).clone())
    }

    pub fn keep(&self, box_id: &str, live: &[String]) {
        if let Ok(mut labels) = self.0.lock() {
            labels.retain(|(owner, _), target| owner != box_id || live.contains(target));
        }
    }

    pub fn forget(&self, box_id: &str) {
        if let Ok(mut labels) = self.0.lock() {
            labels.retain(|(owner, _), _| owner != box_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_label_names_a_tab_of_one_box_until_the_tab_is_gone() {
        let labels = Labels::default();
        labels.set("box-1", "docs", "AAAA");
        labels.set("box-2", "docs", "BBBB");

        assert_eq!(labels.target("box-1", "docs").as_deref(), Some("AAAA"));
        assert_eq!(labels.target("box-2", "docs").as_deref(), Some("BBBB"));
        assert_eq!(labels.of("box-1", "AAAA").as_deref(), Some("docs"));
        assert_eq!(
            labels.target("box-1", "AAAA"),
            None,
            "a word that is no label is left to be read as an id"
        );

        labels.keep("box-1", &["CCCC".to_string()]);
        assert_eq!(
            labels.target("box-1", "docs"),
            None,
            "a label that outlived its tab would send the next action to a tab that is not there"
        );
        assert_eq!(labels.target("box-2", "docs").as_deref(), Some("BBBB"));

        labels.forget("box-2");
        assert_eq!(labels.target("box-2", "docs"), None);
    }

    #[test]
    fn test_a_label_moves_to_the_tab_it_was_last_given() {
        let labels = Labels::default();
        labels.set("box-1", "docs", "AAAA");
        labels.set("box-1", "docs", "BBBB");

        assert_eq!(labels.target("box-1", "docs").as_deref(), Some("BBBB"));
        assert_eq!(labels.of("box-1", "AAAA"), None);
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
