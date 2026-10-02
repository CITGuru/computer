//! The viewer socket, proxied: a page reaches a box's screen through this server rather
//! than through a port of its own, so the box stays on loopback and the server decides
//! who gets in.

use crate::AppState;
use crate::caller::{Caller, Role};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiPath, ApiQuery};
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use axum::{Extension, Json};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use computer_api::{ErrorCode, ViewerTicket};
use futures_util::{SinkExt, StreamExt};
use ring::hmac;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{self, protocol::WebSocketConfig};

/// Long enough to press a button on a page that has sat open a while; short enough that a
/// link pasted somewhere goes stale.
pub const TICKET_LIFE: Duration = Duration::from_secs(15 * 60);

/// What noVNC asks for, and what websockify serves.
const SUBPROTOCOL: &str = "binary";

pub struct Tickets {
    key: hmac::Key,
    forgotten: Mutex<HashSet<String>>,
}

impl Default for Tickets {
    fn default() -> Self {
        let mut bytes = [0u8; 32];
        let _ = getrandom::fill(&mut bytes);
        Self::keyed(&bytes)
    }
}

#[derive(Serialize, Deserialize)]
struct Ticket {
    #[serde(rename = "b")]
    box_id: String,
    #[serde(rename = "s")]
    screen: u32,
    #[serde(rename = "c")]
    control: bool,
    #[serde(rename = "e")]
    until_ms: u64,
    #[serde(rename = "n")]
    nonce: u64,
}

impl Tickets {
    pub fn keyed(key: &[u8]) -> Self {
        Self {
            key: hmac::Key::new(hmac::HMAC_SHA256, key),
            forgotten: Mutex::new(HashSet::new()),
        }
    }

    pub fn mint(&self, box_id: &str, screen: u32) -> ApiResult<(String, SystemTime)> {
        self.mint_for(box_id, screen, TICKET_LIFE)
    }

    pub fn mint_watching(&self, box_id: &str, screen: u32) -> ApiResult<(String, SystemTime)> {
        self.minted(box_id, screen, TICKET_LIFE, false)
    }

    pub fn mint_for(
        &self,
        box_id: &str,
        screen: u32,
        life: Duration,
    ) -> ApiResult<(String, SystemTime)> {
        self.minted(box_id, screen, life, true)
    }

    fn minted(
        &self,
        box_id: &str,
        screen: u32,
        life: Duration,
        control: bool,
    ) -> ApiResult<(String, SystemTime)> {
        let until = SystemTime::now() + life;
        let mut nonce = [0u8; 8];
        getrandom::fill(&mut nonce)
            .map_err(|error| ApiError::internal(format!("no randomness for a ticket: {error}")))?;
        let ticket = Ticket {
            box_id: box_id.to_string(),
            screen,
            control,
            until_ms: crate::routes::ms_of(until),
            nonce: u64::from_le_bytes(nonce),
        };
        let body = serde_json::to_vec(&ticket)
            .map_err(|error| ApiError::internal(format!("a ticket: {error}")))?;
        let payload = URL_SAFE_NO_PAD.encode(body);
        let tag = hmac::sign(&self.key, payload.as_bytes());

        Ok((
            format!("{payload}.{}", URL_SAFE_NO_PAD.encode(tag.as_ref())),
            until,
        ))
    }

    fn read(&self, ticket: &str) -> Option<Ticket> {
        let (payload, tag) = ticket.split_once('.')?;
        let tag = URL_SAFE_NO_PAD.decode(tag).ok()?;
        hmac::verify(&self.key, payload.as_bytes(), &tag).ok()?;

        let found: Ticket = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
        let live = found.until_ms > crate::routes::ms_of(SystemTime::now());
        let kept = self
            .forgotten
            .lock()
            .map(|forgotten| !forgotten.contains(&found.box_id))
            .unwrap_or(false);
        (live && kept).then_some(found)
    }

    pub fn admits(&self, ticket: &str, box_id: &str, screen: u32) -> bool {
        self.read(ticket)
            .is_some_and(|found| found.box_id == box_id && found.screen == screen)
    }

    pub fn controls(&self, ticket: &str) -> bool {
        self.read(ticket).is_some_and(|found| found.control)
    }

    pub fn box_of(&self, ticket: &str) -> Option<String> {
        self.read(ticket).map(|found| found.box_id)
    }

    pub fn forget(&self, box_id: &str) {
        if let Ok(mut forgotten) = self.forgotten.lock() {
            forgotten.insert(box_id.to_string());
        }
    }
}

pub const DOOR_LIFE: Duration = Duration::from_secs(60);

pub fn takeover_life(created: SystemTime, expires: Option<SystemTime>) -> Duration {
    let whole = expires
        .and_then(|ends| ends.duration_since(created).ok())
        .unwrap_or(Duration::from_secs(crate::runtimes::LIFETIME_SECS));

    whole / 2
}

#[derive(Default)]
pub struct Doors {
    key: Option<hmac::Key>,
}

impl Doors {
    pub fn keyed(key: &[u8]) -> Self {
        Self {
            key: Some(hmac::Key::new(hmac::HMAC_SHA256, key)),
        }
    }

    fn box_bytes(&self, box_id: &str) -> Option<Vec<u8>> {
        let key = self.key.as_ref()?;
        Some(hmac::sign(key, box_id.as_bytes()).as_ref().to_vec())
    }

    pub fn box_key(&self, box_id: &str) -> Option<computer::Secret> {
        let bytes = self.box_bytes(box_id)?;
        computer::Secret::new(URL_SAFE_NO_PAD.encode(bytes)).ok()
    }

    pub fn token(&self, box_id: &str, port: u16, life: Duration) -> Option<String> {
        let key = hmac::Key::new(hmac::HMAC_SHA256, &self.box_bytes(box_id)?);
        let until = crate::routes::ms_of(SystemTime::now() + life) / 1000;
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
        let claims = serde_json::json!({ "host": "127.0.0.1", "port": port, "exp": until });
        let signed = format!("{header}.{}", URL_SAFE_NO_PAD.encode(claims.to_string()));
        let tag = hmac::sign(&key, signed.as_bytes());

        Some(format!("{signed}.{}", URL_SAFE_NO_PAD.encode(tag.as_ref())))
    }
}

pub async fn ticket(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    ApiPath((id, screen)): ApiPath<(String, u32)>,
) -> ApiResult<Json<ViewerTicket>> {
    let entry = state.entry(&id).await?;
    let target = entry.desktop(screen).await?;
    let member = caller.may(Role::Member);

    let (ticket, until) = match member {
        true => state.tickets.mint(&id, screen)?,
        false => state.tickets.mint_watching(&id, screen)?,
    };

    let signed = entry.computer.viewer_auth() == computer::Auth::Signed;
    let direct = |control: bool| {
        let held = target
            .as_screen()
            .filter(|held| held.socket_headers().is_empty())?;
        let token = state
            .doors
            .token(&id, held.door_port(control), TICKET_LIFE)?;
        held.signed_socket(control, &computer::Secret::new(token).ok()?)
    };

    Ok(Json(ViewerTicket {
        ticket,
        expires_at_ms: crate::routes::ms_of(until),
        view_socket: signed.then(|| direct(false)).flatten(),
        control_socket: (signed && member).then(|| direct(true)).flatten(),
    }))
}

#[derive(Debug, Deserialize)]
pub struct SocketQuery {
    ticket: String,
    #[serde(default)]
    mode: Mode,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
    #[default]
    View,
    Control,
}

pub async fn socket(
    State(state): State<std::sync::Arc<AppState>>,
    ApiPath((id, screen)): ApiPath<(String, u32)>,
    ApiQuery(query): ApiQuery<SocketQuery>,
    upgrade: WebSocketUpgrade,
) -> ApiResult<Response> {
    if !state.tickets.admits(&query.ticket, &id, screen) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            ErrorCode::Denied,
            "this ticket does not open this screen, or it has expired",
        ));
    }
    if matches!(query.mode, Mode::Control) && !state.tickets.controls(&query.ticket) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            ErrorCode::Denied,
            "this ticket was given to a viewer, who may watch the screen but not drive it",
        ));
    }

    let entry = state.entry(&id).await?;
    let target = entry.desktop(screen).await?;
    let held = target
        .as_screen()
        .ok_or_else(|| ApiError::internal("this screen has no viewer"))?;
    let control = matches!(query.mode, Mode::Control);
    let inside = match entry.computer.viewer_auth() {
        computer::Auth::Signed => state
            .doors
            .token(&id, held.door_port(control), DOOR_LIFE)
            .and_then(|token| computer::Secret::new(token).ok())
            .and_then(|token| held.signed_socket(control, &token)),
        _ if control => held.control_socket(),
        _ => held.viewer_socket(),
    }
    .ok_or_else(|| ApiError::not_found("this screen publishes no viewer port"))?;

    let mut request = inside
        .as_str()
        .into_client_request()
        .map_err(|error| ApiError::internal(format!("the viewer socket address: {error}")))?;
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static(SUBPROTOCOL),
    );
    for (name, value) in held.socket_headers() {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| ApiError::internal(format!("a viewer header name: {error}")))?;
        let value = HeaderValue::from_str(&value)
            .map_err(|error| ApiError::internal(format!("a viewer header value: {error}")))?;
        request.headers_mut().insert(name, value);
    }

    // Connected before the upgrade, so a box that refuses answers with a status and a
    // reason rather than a socket that closes at once.
    let wire = computer::cdp::dial(&inside).await?;
    let (box_side, _) = tokio_tungstenite::client_async_with_config(
        request,
        wire,
        Some(WebSocketConfig::default()),
    )
    .await
    .map_err(|error| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            ErrorCode::Unavailable,
            format!("the box's viewer refused the connection: {error}"),
        )
    })?;

    Ok(upgrade
        .protocols([SUBPROTOCOL])
        .on_upgrade(move |person| carry(person, box_side)))
}

pub(crate) type BoxSide = tokio_tungstenite::WebSocketStream<computer::cdp::Wire>;

async fn carry(mut person: WebSocket, mut box_side: BoxSide) {
    loop {
        tokio::select! {
            from_person = person.recv() => {
                let Some(Ok(message)) = from_person else { break };
                let Some(forward) = inward(message) else { break };
                if box_side.send(forward).await.is_err() {
                    break;
                }
            }
            from_box = box_side.next() => {
                let Some(Ok(message)) = from_box else { break };
                let Some(forward) = outward(message) else { break };
                if person.send(forward).await.is_err() {
                    break;
                }
            }
        }
    }

    let _ = person.send(Message::Close(None)).await;
    let _ = box_side.close(None).await;
}

fn inward(message: Message) -> Option<tungstenite::Message> {
    Some(match message {
        Message::Binary(bytes) => tungstenite::Message::Binary(bytes),
        Message::Text(text) => tungstenite::Message::Text(text.as_str().into()),
        Message::Ping(bytes) => tungstenite::Message::Ping(bytes),
        Message::Pong(bytes) => tungstenite::Message::Pong(bytes),
        Message::Close(_) => return None,
    })
}

fn outward(message: tungstenite::Message) -> Option<Message> {
    Some(match message {
        tungstenite::Message::Binary(bytes) => Message::Binary(bytes),
        tungstenite::Message::Text(text) => Message::Text(text.as_str().into()),
        tungstenite::Message::Ping(bytes) => Message::Ping(bytes),
        tungstenite::Message::Pong(bytes) => Message::Pong(bytes),
        tungstenite::Message::Close(_) | tungstenite::Message::Frame(_) => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_ticket_opens_its_own_screen_and_no_other() {
        let tickets = Tickets::default();
        let (ticket, _) = tickets.mint("box-1", 0).expect("minted");

        assert!(tickets.admits(&ticket, "box-1", 0));
        assert!(!tickets.admits(&ticket, "box-1", 1));
        assert!(!tickets.admits(&ticket, "box-2", 0));
        assert!(!tickets.admits("not-a-ticket", "box-1", 0));
    }

    #[test]
    fn test_a_removed_box_takes_its_tickets_with_it() {
        let tickets = Tickets::default();
        let (ticket, _) = tickets.mint("box-1", 0).expect("minted");

        tickets.forget("box-1");

        assert!(!tickets.admits(&ticket, "box-1", 0));
    }

    #[test]
    fn test_two_tickets_differ() {
        let tickets = Tickets::default();
        let (one, _) = tickets.mint("box-1", 0).expect("minted");
        let (two, _) = tickets.mint("box-1", 0).expect("minted");

        assert_ne!(one, two);
        assert!(tickets.admits(&one, "box-1", 0) && tickets.admits(&two, "box-1", 0));
    }

    #[test]
    fn test_a_ticket_is_checked_by_its_key_and_not_by_memory() {
        let (ticket, _) = Tickets::keyed(b"key one").mint("box-1", 0).expect("minted");

        assert!(
            Tickets::keyed(b"key one").admits(&ticket, "box-1", 0),
            "another process with the same key takes it"
        );
        assert!(!Tickets::keyed(b"key two").admits(&ticket, "box-1", 0));

        let (payload, tag) = ticket.split_once('.').expect("two parts");
        let mut body: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).expect("base64"))
                .expect("json");
        body["b"] = "box-2".into();
        let changed = format!("{}.{tag}", URL_SAFE_NO_PAD.encode(body.to_string()));
        assert!(!Tickets::keyed(b"key one").admits(&changed, "box-2", 0));
    }

    #[test]
    fn test_a_takeover_link_lasts_half_the_life_of_its_box() {
        let made = SystemTime::now();

        assert_eq!(
            takeover_life(made, Some(made + Duration::from_secs(4 * 3600))),
            Duration::from_secs(2 * 3600)
        );
        assert_eq!(
            takeover_life(made, None),
            Duration::from_secs(crate::runtimes::LIFETIME_SECS / 2),
            "a box with no end is counted as one of the default lifetime"
        );
    }

    #[test]
    fn test_an_expired_ticket_is_refused() {
        let tickets = Tickets::keyed(b"key one");
        let (ticket, _) = tickets
            .mint_for("box-1", 0, Duration::ZERO)
            .expect("minted");

        assert!(!tickets.admits(&ticket, "box-1", 0));
    }

    #[test]
    fn test_a_door_token_is_a_jwt_the_box_key_signs_for_one_port() {
        let doors = Doors::keyed(b"doors");
        let key = doors.box_key("box-1").expect("a box key");
        let token = doors.token("box-1", 5901, DOOR_LIFE).expect("a token");

        let (signed, tag) = token.rsplit_once('.').expect("three parts");
        let box_key = hmac::Key::new(
            hmac::HMAC_SHA256,
            &URL_SAFE_NO_PAD.decode(key.expose()).expect("base64url"),
        );
        assert!(
            hmac::verify(
                &box_key,
                signed.as_bytes(),
                &URL_SAFE_NO_PAD.decode(tag).expect("base64url")
            )
            .is_ok()
        );

        let claims: serde_json::Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(signed.split_once('.').expect("a payload").1)
                .expect("base64url"),
        )
        .expect("json");
        assert_eq!(claims["host"], "127.0.0.1");
        assert_eq!(claims["port"], 5901);
        assert!(claims["exp"].as_u64().is_some());

        assert_ne!(
            doors
                .box_key("box-2")
                .map(|other| other.expose().to_string()),
            Some(key.expose().to_string()),
            "one box's key opens no other box"
        );
        assert!(Doors::default().token("box-1", 5901, DOOR_LIFE).is_none());
    }
}
