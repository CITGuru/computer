use serde::Deserialize;
use std::sync::{OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub const HEADER: &str = "x-vercel-oidc-token";

pub const TOKEN_ENV: &str = "VERCEL_OIDC_TOKEN";

pub const ON_VERCEL_ENV: &str = "VERCEL";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Oidc {
    pub token: String,
    pub team: String,
    pub project: String,
    pub team_slug: Option<String>,
    pub project_name: Option<String>,
    issued_at: u64,
    expires_at: u64,
}

#[derive(Deserialize)]
struct Claims {
    owner_id: String,
    project_id: String,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    iat: u64,
    #[serde(default)]
    exp: u64,
}

impl Oidc {
    pub fn read(token: &str) -> Option<Self> {
        let payload = token.split('.').nth(1)?;
        let claims: Claims = serde_json::from_slice(&base64url(payload)?).ok()?;

        Some(Self {
            token: token.to_string(),
            team: claims.owner_id,
            project: claims.project_id,
            team_slug: claims.owner,
            project_name: claims.project,
            issued_at: claims.iat,
            expires_at: claims.exp,
        })
    }

    fn live(&self, now: u64) -> bool {
        self.expires_at == 0 || self.expires_at > now
    }
}

pub fn on_vercel() -> bool {
    std::env::var(ON_VERCEL_ENV).is_ok_and(|value| value == "1")
}

pub fn possible() -> bool {
    on_vercel() || std::env::var(TOKEN_ENV).is_ok_and(|value| !value.is_empty())
}

fn slot() -> &'static RwLock<Option<Oidc>> {
    static SLOT: OnceLock<RwLock<Option<Oidc>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

pub fn offer(token: &str) -> bool {
    if !on_vercel() {
        return false;
    }
    let Some(offered) = Oidc::read(token) else {
        return false;
    };
    let Ok(mut held) = slot().write() else {
        return false;
    };

    match keep(held.as_ref(), offered, pinned_team().as_deref()) {
        Some(newer) => {
            *held = Some(newer);
            true
        }
        None => false,
    }
}

fn pinned_team() -> Option<String> {
    std::env::var(super::cloud::TEAM_ENV)
        .ok()
        .filter(|team| !team.is_empty())
}

fn keep(held: Option<&Oidc>, offered: Oidc, team: Option<&str>) -> Option<Oidc> {
    if team.is_some_and(|team| team != offered.team) {
        return None;
    }

    match held {
        None => Some(offered),
        Some(held) if held.team != offered.team || held.project != offered.project => None,
        Some(held) if offered.issued_at <= held.issued_at => None,
        Some(_) => Some(offered),
    }
}

pub fn current() -> Option<Oidc> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default();

    let held = slot().read().ok().and_then(|held| held.clone());
    let from_env = || {
        std::env::var(TOKEN_ENV)
            .ok()
            .and_then(|token| Oidc::read(&token))
    };

    held.or_else(from_env).filter(|oidc| oidc.live(now))
}

fn base64url(text: &str) -> Option<Vec<u8>> {
    let mut standard: String = text
        .chars()
        .map(|one| match one {
            '-' => '+',
            '_' => '/',
            other => other,
        })
        .collect();
    while standard.len() % 4 != 0 {
        standard.push('=');
    }

    crate::cdp::base64_decode(&standard)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(team: &str, project: &str, iat: u64) -> String {
        let claims = serde_json::json!({
            "owner_id": team,
            "project_id": project,
            "owner": "acme",
            "project": "desk",
            "iat": iat,
            "exp": iat + 7200,
        });
        let payload = crate::cdp::base64_encode(claims.to_string().as_bytes())
            .trim_end_matches('=')
            .replace('+', "-")
            .replace('/', "_");

        format!("e30.{payload}.signature")
    }

    fn read(team: &str, project: &str, iat: u64) -> Oidc {
        Oidc::read(&token(team, project, iat)).expect("a token")
    }

    #[test]
    fn test_the_team_and_project_come_from_the_claims() {
        let oidc = read("team_a", "prj_a", 100);

        assert_eq!(oidc.team, "team_a");
        assert_eq!(oidc.project, "prj_a");
        assert_eq!(oidc.team_slug.as_deref(), Some("acme"));
        assert_eq!(oidc.project_name.as_deref(), Some("desk"));
        assert!(oidc.live(101));
        assert!(!oidc.live(100 + 7200));
    }

    #[test]
    fn test_what_is_not_a_token_is_refused() {
        assert_eq!(Oidc::read("not-a-token"), None);
        assert_eq!(Oidc::read("a.b.c"), None);
    }

    #[test]
    fn test_a_newer_token_for_the_same_project_replaces_the_held_one() {
        let held = read("team_a", "prj_a", 100);

        assert_eq!(
            keep(Some(&held), read("team_a", "prj_a", 200), None),
            Some(read("team_a", "prj_a", 200))
        );
        assert_eq!(keep(Some(&held), read("team_a", "prj_a", 50), None), None);
    }

    #[test]
    fn test_a_token_for_another_project_or_team_is_refused() {
        let held = read("team_a", "prj_a", 100);

        assert_eq!(keep(Some(&held), read("team_a", "prj_b", 200), None), None);
        assert_eq!(keep(Some(&held), read("team_b", "prj_a", 200), None), None);
        assert_eq!(
            keep(None, read("team_b", "prj_a", 200), Some("team_a")),
            None
        );
    }
}
