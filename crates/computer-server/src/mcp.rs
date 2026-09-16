//! MCP over Streamable HTTP, on the same address as the REST API. Each request is
//! answered by the MCP crate driving this server through its own REST routes, so the
//! bearer a caller offers is the bearer the tools run under.

use crate::AppState;
use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use computer_client::Client;
use serde_json::{Value, json};
use std::sync::Arc;

pub struct Mcp {
    /// Where this server answers on the loopback, for the tools to call back into.
    api: String,
    /// The origin a browser reaches this server at, when a proxy in front hides it.
    public: Option<String>,
}

pub fn router(state: Arc<AppState>, api: String, public: Option<String>) -> Router {
    let mcp = Arc::new(Mcp {
        api: api.trim_end_matches('/').to_string(),
        public: public.map(|url| url.trim_end_matches('/').to_string()),
    });

    Router::new()
        .route("/mcp", post(answer).get(no_stream).delete(ended))
        .layer(axum::middleware::from_fn_with_state(
            state,
            crate::auth::gate,
        ))
        .with_state(mcp)
}

async fn answer(State(mcp): State<Arc<Mcp>>, headers: HeaderMap, body: Bytes) -> Response {
    if let Some(refused) = foreign(&headers) {
        return refused;
    }

    let message: Value = match serde_json::from_slice(&body) {
        Ok(message) => message,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": { "code": -32700, "message": format!("not JSON-RPC: {error}") },
                })),
            )
                .into_response();
        }
    };

    let mut client = Client::new(&mcp.api);
    if let Some(bearer) = bearer(&headers) {
        client = client.with_token(bearer);
    }
    let server =
        computer_mcp::Server::new(client).reached_at(origin(&headers, mcp.public.as_deref()));

    let batch = message.is_array();
    let mut answers = Vec::new();
    for one in match message {
        Value::Array(many) => many,
        one => vec![one],
    } {
        if let Some(answered) = server.handle(one).await {
            answers.push(answered);
        }
    }

    match (answers.is_empty(), batch) {
        (true, _) => StatusCode::ACCEPTED.into_response(),
        (false, true) => axum::Json(Value::Array(answers)).into_response(),
        (false, false) => axum::Json(answers.remove(0)).into_response(),
    }
}

/// Nothing here starts a message of its own, so there is no stream to open.
async fn no_stream() -> Response {
    StatusCode::METHOD_NOT_ALLOWED.into_response()
}

/// Sessions hold nothing, so ending one is already done.
async fn ended() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

/// A browser page on another site must not reach a loopback server through the person's
/// browser: an `Origin` that is neither this host nor a loopback is refused.
fn foreign(headers: &HeaderMap) -> Option<Response> {
    let origin = header(headers, header::ORIGIN)?;
    let host_of = |url: &str| {
        url.split("://")
            .nth(1)
            .unwrap_or(url)
            .split('/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
    };
    let claimed = host_of(&origin);
    let ours = header(headers, header::HOST)
        .map(|host| host.to_ascii_lowercase())
        .unwrap_or_default();
    let loopback = ["localhost", "127.0.0.1", "[::1]"]
        .iter()
        .any(|name| claimed == *name || claimed.starts_with(&format!("{name}:")));

    if claimed == ours || loopback {
        return None;
    }

    Some(
        (
            StatusCode::FORBIDDEN,
            axum::Json(json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": { "code": -32000, "message": format!("{origin} may not reach this server") },
            })),
        )
            .into_response(),
    )
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    header(headers, header::AUTHORIZATION)?
        .strip_prefix("Bearer ")
        .map(str::to_string)
}

/// The origin a page will connect back to: told by the operator, else read off the
/// proxy's headers, else the address this request arrived on.
fn origin(headers: &HeaderMap, public: Option<&str>) -> String {
    if let Some(public) = public {
        return public.to_string();
    }

    let scheme = header(headers, "x-forwarded-proto").unwrap_or_else(|| "http".to_string());
    let host = header(headers, "x-forwarded-host")
        .or_else(|| header(headers, header::HOST))
        .unwrap_or_else(|| "127.0.0.1".to_string());

    format!("{scheme}://{host}")
}

fn header<K: axum::http::header::AsHeaderName>(headers: &HeaderMap, name: K) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(',').next().unwrap_or(value).trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn with(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(*name, HeaderValue::from_str(value).expect("a header"));
        }
        headers
    }

    #[test]
    fn test_the_origin_is_the_operators_word_first() {
        let headers = with(&[("host", "127.0.0.1:8181")]);

        assert_eq!(
            origin(&headers, Some("https://boxes.example.com")),
            "https://boxes.example.com"
        );
    }

    #[test]
    fn test_the_origin_is_read_off_a_proxy_before_the_host() {
        let headers = with(&[
            ("host", "127.0.0.1:8181"),
            ("x-forwarded-proto", "https"),
            ("x-forwarded-host", "boxes.example.com"),
        ]);

        assert_eq!(origin(&headers, None), "https://boxes.example.com");
    }

    #[test]
    fn test_the_origin_falls_back_to_where_the_request_arrived() {
        let headers = with(&[("host", "127.0.0.1:8181")]);

        assert_eq!(origin(&headers, None), "http://127.0.0.1:8181");
    }

    #[test]
    fn test_a_page_from_elsewhere_is_refused_and_a_local_one_is_not() {
        let ours = with(&[
            ("host", "127.0.0.1:8181"),
            ("origin", "http://127.0.0.1:8181"),
        ]);
        let local = with(&[
            ("host", "127.0.0.1:8181"),
            ("origin", "http://localhost:5173"),
        ]);
        let elsewhere = with(&[
            ("host", "127.0.0.1:8181"),
            ("origin", "https://evil.example"),
        ]);
        let none = with(&[("host", "127.0.0.1:8181")]);

        assert!(foreign(&ours).is_none());
        assert!(foreign(&local).is_none());
        assert!(foreign(&elsewhere).is_some());
        assert!(foreign(&none).is_none());
    }
}
