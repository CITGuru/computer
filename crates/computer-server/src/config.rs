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
                "context" if provider == "docker" => tuning.context = Some(text(&at, value)?),
                "context" => {
                    return Err(format!(
                        "{at} is a docker field and runtimes.{name} is a {provider} runtime"
                    ));
                }
                _ => {
                    return Err(format!(
                        "{at} is not a field a {provider} runtime takes: provider, enabled, \
                         memory, cpus, isolation{}",
                        match provider {
                            "docker" => ", context",
                            _ => "",
                        }
                    ));
                }
            }
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
