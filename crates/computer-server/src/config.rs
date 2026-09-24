use crate::runtimes::{HOSTS, Tuning};
use serde::Deserialize;
use std::collections::BTreeMap;

pub const FILE: &str = "COMPUTER_SERVER_CONFIG";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub runtimes: BTreeMap<String, RuntimeEntry>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RuntimeEntry {
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(flatten)]
    pub fields: toml::Table,
}

impl ServerConfig {
    pub fn from_env() -> Result<Self, String> {
        let Some(path) = std::env::var(FILE).ok().filter(|path| !path.is_empty()) else {
            return Ok(Self::default());
        };

        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("{FILE}={path} and it cannot be read: {error}"))?;

        tracing::info!(%path, "reading the runtimes from");
        Self::parse(&text).map_err(|why| format!("{path}: {why}"))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|error| error.message().to_string())
    }
}

impl RuntimeEntry {
    pub fn tuning(&self, name: &str, provider: &str) -> Result<Tuning, String> {
        let mut tuning = Tuning::default();

        for (key, value) in &self.fields {
            let at = format!("runtimes.{name}.{key}");

            match key.as_str() {
                "memory" => tuning.memory = Some(text(&at, value)?),
                "cpus" => tuning.cpus = Some(number(&at, value)?),
                "isolation" => tuning.isolation = Some(text(&at, value)?),
                "lifetime" => tuning.lifetime_secs = Some(seconds(&at, value)?),
                "max_lifetime" => tuning.max_lifetime_secs = Some(seconds(&at, value)?),
                "context" if provider == "docker" => tuning.context = Some(text(&at, value)?),
                "program" if crate::runtimes::MICROVMS.contains(&provider) => {
                    tuning.program = Some(text(&at, value)?)
                }
                "build_with" if crate::runtimes::MICROVMS.contains(&provider) => {
                    tuning.builds_with = Some(text(&at, value)?)
                }
                "build_with" => {
                    return Err(format!(
                        "{at} is a hypervisor's field and runtimes.{name} is a {provider} runtime"
                    ));
                }
                "program" => {
                    return Err(format!(
                        "{at} is a hypervisor's field and runtimes.{name} is a {provider} runtime"
                    ));
                }
                "context" => {
                    return Err(format!(
                        "{at} is a docker field and runtimes.{name} is a {provider} runtime"
                    ));
                }
                _ => {
                    return Err(format!(
                        "{at} is not a field a {provider} runtime takes: provider, enabled, \
                         memory, cpus, isolation, lifetime, max_lifetime{}",
                        match provider {
                            "docker" => ", context",
                            other if crate::runtimes::MICROVMS.contains(&other) =>
                                ", program, build_with",
                            _ => "",
                        }
                    ));
                }
            }
        }

        if let (Some(life), Some(most)) = (tuning.lifetime_secs, tuning.max_lifetime_secs)
            && life > most
        {
            return Err(format!(
                "runtimes.{name}.lifetime is {life}s and its max_lifetime is {most}s, \
                 so every box here would be refused"
            ));
        }

        Ok(tuning)
    }

    pub fn host(&self, name: &str) -> Result<String, String> {
        let provider = self.provider.clone().unwrap_or_else(|| name.to_string());

        match HOSTS.contains(&provider.as_str()) {
            true => Ok(provider),
            false => Err(format!(
                "runtimes.{name} is a {provider} runtime, and a file configures host \
                 runtimes only: {}",
                HOSTS.join(", ")
            )),
        }
    }
}

fn text(at: &str, value: &toml::Value) -> Result<String, String> {
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("{at} is {value} and it is written as text"))
}

fn seconds(at: &str, value: &toml::Value) -> Result<u64, String> {
    let said = match value {
        toml::Value::Integer(whole) if *whole > 0 => return Ok(*whole as u64),
        toml::Value::String(said) => said.trim().to_string(),
        _ => {
            return Err(format!(
                "{at} is {value} and it is written as 24h, 90m, 3600s, or a whole \
                 number of seconds"
            ));
        }
    };

    let (count, scale) = match said.chars().last() {
        Some('h') => (said.trim_end_matches('h'), 60 * 60),
        Some('m') => (said.trim_end_matches('m'), 60),
        Some('s') => (said.trim_end_matches('s'), 1),
        _ => (said.as_str(), 1),
    };

    count
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|count| *count > 0)
        .map(|count| count * scale)
        .ok_or_else(|| {
            format!("{at} is {said:?} and it is written as 24h, 90m, 3600s, or a whole number of seconds")
        })
}

fn number(at: &str, value: &toml::Value) -> Result<String, String> {
    match value {
        toml::Value::Integer(whole) => Ok(whole.to_string()),
        toml::Value::Float(part) => Ok(part.to_string()),
        toml::Value::String(said) => Ok(said.clone()),
        _ => Err(format!("{at} is {value} and it is written as a number")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_file_says_what_to_offer_and_which_to_use() {
        let config = ServerConfig::parse(
            "default = \"hardened\"\n\
             \n\
             [runtimes.docker]\n\
             memory = \"4g\"\n\
             \n\
             [runtimes.hardened]\n\
             provider = \"docker\"\n\
             isolation = \"runsc\"\n\
             context = \"gpu-1\"\n",
        )
        .expect("a file this server reads");

        assert_eq!(config.default.as_deref(), Some("hardened"));
        assert_eq!(config.runtimes.len(), 2);

        let tuning = config.runtimes["hardened"]
            .tuning("hardened", "docker")
            .expect("fields docker takes");
        assert_eq!(tuning.isolation.as_deref(), Some("runsc"));
        assert_eq!(tuning.context.as_deref(), Some("gpu-1"));
    }

    #[test]
    fn test_cpus_are_read_however_they_are_written() {
        for written in ["cpus = 2", "cpus = 1.5", "cpus = \"2\""] {
            let config = ServerConfig::parse(&format!("[runtimes.docker]\n{written}\n"))
                .expect("a file this server reads");

            assert!(
                config.runtimes["docker"]
                    .tuning("docker", "docker")
                    .expect("a number")
                    .cpus
                    .is_some(),
                "{written}"
            );
        }
    }

    #[test]
    fn test_memory_written_as_a_number_is_refused_rather_than_guessed() {
        let config = ServerConfig::parse("[runtimes.docker]\nmemory = 4\n").expect("read");

        let why = config.runtimes["docker"]
            .tuning("docker", "docker")
            .expect_err("4 of what");

        assert!(why.contains("runtimes.docker.memory"), "{why}");
    }

    #[test]
    fn test_a_key_outside_the_runtimes_is_refused() {
        assert!(
            ServerConfig::parse("defualt = \"docker\"\n").is_err(),
            "a misspelled key that quietly does nothing is worse than a refusal"
        );
    }

    #[test]
    fn test_an_empty_file_offers_what_the_server_finds() {
        let config = ServerConfig::parse("").expect("a file that says nothing");

        assert!(config.default.is_none());
        assert!(config.runtimes.is_empty());
    }
}

#[cfg(test)]
mod lives {
    use super::*;

    fn tuning(text: &str) -> Result<crate::runtimes::Tuning, String> {
        let config = ServerConfig::parse(&format!("[runtimes.docker]\n{text}\n")).expect("read");

        config.runtimes["docker"].tuning("docker", "docker")
    }

    #[test]
    fn test_a_life_is_written_the_way_people_write_one() {
        assert_eq!(
            tuning("max_lifetime = \"24h\"")
                .expect("hours")
                .max_lifetime_secs,
            Some(24 * 60 * 60)
        );
        assert_eq!(
            tuning("lifetime = \"90m\"").expect("minutes").lifetime_secs,
            Some(90 * 60)
        );
        assert_eq!(
            tuning("lifetime = \"3600s\"")
                .expect("seconds")
                .lifetime_secs,
            Some(3600)
        );
        assert_eq!(
            tuning("lifetime = 3600").expect("a number").lifetime_secs,
            Some(3600)
        );
    }

    #[test]
    fn test_a_life_nothing_can_read_is_refused() {
        for written in [
            "lifetime = \"soon\"",
            "lifetime = \"0h\"",
            "lifetime = true",
        ] {
            assert!(tuning(written).is_err(), "{written}");
        }
    }

    #[test]
    fn test_a_default_life_longer_than_the_cap_is_refused_at_the_start() {
        let why = tuning("lifetime = \"2h\"\nmax_lifetime = \"1h\"")
            .expect_err("every box here would be refused");

        assert!(why.contains("lifetime"), "{why}");
    }
}
