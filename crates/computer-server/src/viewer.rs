//! The viewer socket, proxied: a page reaches a box's screen through this server rather
//! than through a port of its own, so the box stays on loopback and the server decides
//! who gets in.

use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiPath, ApiQuery};
use axum::Json;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderValue, StatusCode};
use axum::response::Response;
use computer_api::{ErrorCode, ViewerTicket};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{self, protocol::WebSocketConfig};

/// Long enough to press a button on a page that has sat open a while; short enough that a
/// link pasted somewhere goes stale.
pub const TICKET_LIFE: Duration = Duration::from_secs(15 * 60);

/// What noVNC asks for, and what websockify serves.
const SUBPROTOCOL: &str = "binary";

#[derive(Default)]
pub struct Tickets {
    held: Mutex<HashMap<String, Ticket>>,
}

struct Ticket {
    box_id: String,
    screen: u32,
    until: SystemTime,
}

impl Tickets {
    pub fn mint(&self, box_id: &str, screen: u32) -> ApiResult<(String, SystemTime)> {
        self.mint_for(box_id, screen, TICKET_LIFE)
    }

    pub fn mint_for(
        &self,
        box_id: &str,
        screen: u32,
        life: Duration,
    ) -> ApiResult<(String, SystemTime)> {
        let secret = computer::Secret::generate()?;
        let now = SystemTime::now();
        let until = now + life;

        let mut held = self.lock()?;
        held.retain(|_, ticket| ticket.until > now);
        held.insert(
            secret.expose().to_string(),
            Ticket {
                box_id: box_id.to_string(),
                screen,
                until,
            },
        );

        Ok((secret.expose().to_string(), until))
    }

    pub fn admits(&self, ticket: &str, box_id: &str, screen: u32) -> bool {
        let Ok(held) = self.held.lock() else {
            return false;
        };

        held.get(ticket).is_some_and(|found| {
            found.box_id == box_id && found.screen == screen && found.until > SystemTime::now()
        })
    }

    pub fn box_of(&self, ticket: &str) -> Option<String> {
        let held = self.held.lock().ok()?;

        held.get(ticket)
            .filter(|found| found.until > SystemTime::now())
            .map(|found| found.box_id.clone())
    }

    pub fn forget(&self, box_id: &str) {
        if let Ok(mut held) = self.held.lock() {
            held.retain(|_, ticket| ticket.box_id != box_id);
        }
    }

    fn lock(&self) -> ApiResult<std::sync::MutexGuard<'_, HashMap<String, Ticket>>> {
        self.held
            .lock()
            .map_err(|_| ApiError::internal("the ticket table was poisoned"))
    }
}

pub async fn ticket(
    State(state): State<std::sync::Arc<AppState>>,
    ApiPath((id, screen)): ApiPath<(String, u32)>,
) -> ApiResult<Json<ViewerTicket>> {
    let entry = state.registry.get(&id).await?;
    entry.desktop(screen).await?;

    let (ticket, until) = state.tickets.mint(&id, screen)?;
    Ok(Json(ViewerTicket {
        ticket,
        expires_at_ms: crate::routes::ms_of(until),
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

    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;
    let held = target
        .as_screen()
        .ok_or_else(|| ApiError::internal("this screen has no viewer"))?;
    let inside = match query.mode {
        Mode::View => held.viewer_socket(),
        Mode::Control => held.control_socket(),
    }
    .ok_or_else(|| ApiError::not_found("this screen publishes no viewer port"))?;

    let mut request = inside
        .into_client_request()
        .map_err(|error| ApiError::internal(format!("the viewer socket address: {error}")))?;
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static(SUBPROTOCOL),
    );

    // Connected before the upgrade, so a box that refuses answers with a status and a
    // reason rather than a socket that closes at once.
    let (box_side, _) = tokio_tungstenite::connect_async_with_config(
        request,
        Some(WebSocketConfig::default()),
        false,
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

pub(crate) type BoxSide =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

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
}
