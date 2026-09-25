use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiPath, ApiQuery};
use crate::registry::Entry;
use crate::viewer::BoxSide;
use axum::Json;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use computer_api::{CdpToken, ErrorCode};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{self, protocol::WebSocketConfig};

pub const TOKEN_LIFE: Duration = Duration::from_secs(60 * 60);
const LONGEST_LIFE: Duration = Duration::from_secs(24 * 60 * 60);

const LARGEST_MESSAGE: usize = 256 << 20;

#[derive(Debug, Default, Deserialize)]
pub struct TokenQuery {
    #[serde(default)]
    ttl_secs: Option<u64>,
}

pub async fn token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ApiPath(id): ApiPath<String>,
    ApiQuery(query): ApiQuery<TokenQuery>,
) -> ApiResult<Json<CdpToken>> {
    let entry = state.registry.get(&id).await?;
    let browser = browser_of(&entry)?;

    let life = match query.ttl_secs {
        None => TOKEN_LIFE,
        Some(0) => return Err(ApiError::bad_request("ttl_secs of 0 opens nothing")),
        Some(secs) => Duration::from_secs(secs).min(LONGEST_LIFE),
    };

    let version = browser.version().await?;
    let path = version
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .and_then(path_of)
        .ok_or_else(|| ApiError::internal("the browser named no socket of its own"))?
        .to_string();

    let (token, until) = state.cdp_tokens.mint_for(&id, 0, life)?;
    let outside = Outside::of(&headers, &token)?;

    Ok(Json(CdpToken {
        url: outside.http,
        ws_url: format!("{}{path}", outside.ws),
        expires_at_ms: crate::routes::ms_of(until),
    }))
}

pub async fn json_root(
    State(state): State<Arc<AppState>>,
    method: Method,
    headers: HeaderMap,
    ApiPath(token): ApiPath<String>,
    RawQuery(query): RawQuery,
) -> ApiResult<Response> {
    relayed(&state, &method, &headers, &token, "/json", query).await
}

pub async fn json_under(
    State(state): State<Arc<AppState>>,
    method: Method,
    headers: HeaderMap,
    ApiPath((token, rest)): ApiPath<(String, String)>,
    RawQuery(query): RawQuery,
) -> ApiResult<Response> {
    relayed(
        &state,
        &method,
        &headers,
        &token,
        &format!("/json/{rest}"),
        query,
    )
    .await
}

async fn relayed(
    state: &AppState,
    method: &Method,
    headers: &HeaderMap,
    token: &str,
    path: &str,
    query: Option<String>,
) -> ApiResult<Response> {
    let entry = admitted(state, token).await?;
    let browser = browser_of(&entry)?;
    let outside = Outside::of(headers, token)?;

    let asked = match query {
        Some(query) => format!("{path}?{query}"),
        None => path.to_string(),
    };

    entry.computer.touch();
    let mut said = browser.http(method.as_str(), &asked).await?;
    rehome(&mut said, &outside.ws);

    Ok(match said {
        Value::String(plain) => ([(header::CONTENT_TYPE, "text/plain")], plain).into_response(),
        json => Json(json).into_response(),
    })
}

pub async fn socket(
    State(state): State<Arc<AppState>>,
    ApiPath((token, rest)): ApiPath<(String, String)>,
    upgrade: WebSocketUpgrade,
) -> ApiResult<Response> {
    let entry = admitted(&state, &token).await?;
    let browser = browser_of(&entry)?;

    let mut request = browser
        .socket_url(&format!("/devtools/{rest}"))
        .into_client_request()
        .map_err(|error| ApiError::internal(format!("the DevTools socket address: {error}")))?;
    for (name, value) in browser.headers() {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| ApiError::internal(format!("a DevTools header name: {error}")))?;
        let value = HeaderValue::from_str(value)
            .map_err(|error| ApiError::internal(format!("a DevTools header value: {error}")))?;
        request.headers_mut().insert(name, value);
    }

    let config = WebSocketConfig::default()
        .max_message_size(Some(LARGEST_MESSAGE))
        .max_frame_size(Some(LARGEST_MESSAGE));

    let wire = browser.dial().await?;
    let (box_side, _) = tokio_tungstenite::client_async_with_config(request, wire, Some(config))
        .await
        .map_err(|error| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                ErrorCode::Unavailable,
                format!("the box's browser refused the connection: {error}"),
            )
        })?;

    Ok(upgrade
        .max_message_size(LARGEST_MESSAGE)
        .max_frame_size(LARGEST_MESSAGE)
        .on_upgrade(move |client| carry(client, box_side, entry)))
}

async fn carry(mut client: WebSocket, mut box_side: BoxSide, entry: Arc<Entry>) {
    loop {
        tokio::select! {
            from_client = client.recv() => {
                let Some(Ok(message)) = from_client else { break };
                let forward = match message {
                    Message::Binary(bytes) => tungstenite::Message::Binary(bytes),
                    Message::Text(text) => tungstenite::Message::Text(text.as_str().into()),
                    Message::Ping(bytes) => tungstenite::Message::Ping(bytes),
                    Message::Pong(bytes) => tungstenite::Message::Pong(bytes),
                    Message::Close(_) => break,
                };

                entry.computer.touch();
                if box_side.send(forward).await.is_err() {
                    break;
                }
            }
            from_box = box_side.next() => {
                let Some(Ok(message)) = from_box else { break };
                let forward = match message {
                    tungstenite::Message::Binary(bytes) => Message::Binary(bytes),
                    tungstenite::Message::Text(text) => Message::Text(text.as_str().into()),
                    tungstenite::Message::Ping(bytes) => Message::Ping(bytes),
                    tungstenite::Message::Pong(bytes) => Message::Pong(bytes),
                    tungstenite::Message::Close(_) | tungstenite::Message::Frame(_) => break,
                };

                if client.send(forward).await.is_err() {
                    break;
                }
            }
        }
    }

    let _ = client.send(Message::Close(None)).await;
    let _ = box_side.close(None).await;
}

async fn admitted(state: &AppState, token: &str) -> ApiResult<Arc<Entry>> {
    let id = state.cdp_tokens.box_of(token).ok_or_else(|| {
        ApiError::new(
            StatusCode::FORBIDDEN,
            ErrorCode::Denied,
            "this token opens no browser, or it has expired",
        )
    })?;

    state.registry.get(&id).await
}

fn browser_of(entry: &Entry) -> ApiResult<computer::Devtools> {
    entry.computer.browser().ok_or_else(|| {
        ApiError::not_found("this box publishes no DevTools port for this server to reach")
    })
}

struct Outside {
    http: String,
    ws: String,
}

impl Outside {
    fn of(headers: &HeaderMap, token: &str) -> ApiResult<Self> {
        let host = headers
            .get(header::HOST)
            .and_then(|host| host.to_str().ok())
            .ok_or_else(|| {
                ApiError::bad_request("a CDP address is built from Host, and this request has none")
            })?;

        let secure = headers
            .get("x-forwarded-proto")
            .and_then(|proto| proto.to_str().ok())
            .is_some_and(|proto| proto.eq_ignore_ascii_case("https"));
        let (http, ws) = match secure {
            true => ("https", "wss"),
            false => ("http", "ws"),
        };

        Ok(Self {
            http: format!("{http}://{host}/v1/cdp/{token}"),
            ws: format!("{ws}://{host}/v1/cdp/{token}"),
        })
    }
}

fn path_of(socket: &str) -> Option<&str> {
    let (_, rest) = socket.split_once("://")?;
    rest.find('/').and_then(|at| rest.get(at..))
}

fn rehome(said: &mut Value, ws: &str) {
    match said {
        Value::Array(listed) => listed.iter_mut().for_each(|one| rehome(one, ws)),
        Value::Object(fields) => {
            fields.remove("devtoolsFrontendUrl");

            if let Some(path) = fields
                .get("webSocketDebuggerUrl")
                .and_then(Value::as_str)
                .and_then(path_of)
            {
                let moved = format!("{ws}{path}");
                fields.insert("webSocketDebuggerUrl".to_string(), Value::String(moved));
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const WS: &str = "wss://boxes.example.com/v1/cdp/TOKEN";

    #[test]
    fn test_the_browser_socket_is_pointed_back_through_the_server() {
        let mut version = json!({
            "Browser": "Chrome/152.0.7977.82",
            "webSocketDebuggerUrl": "ws://127.0.0.1:57601/devtools/browser/ff7e0e97",
        });
        rehome(&mut version, WS);

        assert_eq!(
            version["webSocketDebuggerUrl"],
            "wss://boxes.example.com/v1/cdp/TOKEN/devtools/browser/ff7e0e97"
        );
        assert_eq!(version["Browser"], "Chrome/152.0.7977.82");
    }

    #[test]
    fn test_every_page_in_a_list_is_pointed_back_and_no_loopback_address_is_left() {
        let mut pages = json!([
            {
                "id": "ACD6",
                "url": "https://example.com/",
                "devtoolsFrontendUrl": "/devtools/inspector.html?ws=127.0.0.1:57601/devtools/page/ACD6",
                "webSocketDebuggerUrl": "ws://127.0.0.1:57601/devtools/page/ACD6",
            },
            { "id": "C882", "webSocketDebuggerUrl": "ws://127.0.0.1:57601/devtools/page/C882" },
        ]);
        rehome(&mut pages, WS);

        assert_eq!(
            pages[0]["webSocketDebuggerUrl"],
            format!("{WS}/devtools/page/ACD6")
        );
        assert_eq!(
            pages[1]["webSocketDebuggerUrl"],
            format!("{WS}/devtools/page/C882")
        );
        assert_eq!(pages[0]["url"], "https://example.com/");
        assert!(
            !pages.to_string().contains("127.0.0.1"),
            "an address on the box's loopback is one no client out here can use: {pages}"
        );
    }

    #[test]
    fn test_the_address_handed_out_is_the_one_the_client_came_by() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "127.0.0.1:8181".parse().expect("a host"));

        let plain = Outside::of(&headers, "T").expect("a host");
        assert_eq!(plain.http, "http://127.0.0.1:8181/v1/cdp/T");
        assert_eq!(plain.ws, "ws://127.0.0.1:8181/v1/cdp/T");

        headers.insert(header::HOST, "boxes.example.com".parse().expect("a host"));
        headers.insert("x-forwarded-proto", "https".parse().expect("a scheme"));

        let behind_tls = Outside::of(&headers, "T").expect("a host");
        assert_eq!(behind_tls.http, "https://boxes.example.com/v1/cdp/T");
        assert_eq!(behind_tls.ws, "wss://boxes.example.com/v1/cdp/T");

        assert!(Outside::of(&HeaderMap::new(), "T").is_err());
    }

    #[test]
    fn test_a_viewer_ticket_opens_no_browser() {
        let state = AppState::default();

        let (viewer, _) = state.tickets.mint("box-1", 0).expect("a viewer ticket");
        let (cdp, _) = state
            .cdp_tokens
            .mint_for("box-1", 0, TOKEN_LIFE)
            .expect("a CDP token");

        assert_eq!(state.cdp_tokens.box_of(&cdp).as_deref(), Some("box-1"));
        assert_eq!(
            state.cdp_tokens.box_of(&viewer),
            None,
            "a viewer ticket is handed to a page in somebody's chat window, and DevTools \
             reads every cookie the browser holds"
        );

        state.cdp_tokens.forget("box-1");
        assert_eq!(state.cdp_tokens.box_of(&cdp), None);
    }

    #[test]
    fn test_a_token_past_its_life_opens_nothing() {
        let state = AppState::default();
        let (token, _) = state
            .cdp_tokens
            .mint_for("box-1", 0, Duration::ZERO)
            .expect("a token");

        assert_eq!(state.cdp_tokens.box_of(&token), None);
    }
}
