//! The API's answers that do not need a box behind them.
//!
//! Every refusal here is one a client meets before it ever launches
//! anything, so none of them needs a container runtime.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use computer_server::{AppState, routes};
use http_body_util::BodyExt;
use serde_json::Value;
use std::sync::Arc;
use tower::ServiceExt;

async fn send(request: Request<Body>) -> (StatusCode, Value) {
    let router = routes::router(Arc::new(AppState::default()));
    let response = router.oneshot(request).await.expect("the router answered");

    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();

    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "every answer is JSON, and this one was not ({error}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };

    (status, body)
}

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .body(Body::empty())
        .expect("a request")
}

fn post(path: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("a request")
}

#[tokio::test]
async fn test_a_server_holding_nothing_lists_nothing() {
    let (status, body) = send(get("/v1/boxes")).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["boxes"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn test_a_box_that_was_never_here_is_not_found() {
    let (status, body) = send(get("/v1/boxes/box_nope")).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
}

#[tokio::test]
async fn test_removing_a_box_without_saying_so_is_refused() {
    let request = Request::builder()
        .method("DELETE")
        .uri("/v1/boxes/box_nope")
        .body(Body::empty())
        .expect("a request");

    let (status, body) = send(request).await;

    // Refused for the missing header rather than 404 for the missing box: the
    // guard is the point, and a caller that learns the box is gone first will
    // never learn about the header at all.
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("x-computer-confirm-delete"),
        "the refusal names the header it wants: {body}"
    );
}

#[tokio::test]
async fn test_a_spec_naming_apps_is_refused_before_anything_starts() {
    let (status, body) = send(post("/v1/boxes", r#"{"spec":{"apps":{"vscode":{}}}}"#)).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("vscode"),
        "the refusal names the app: {body}"
    );
}

#[tokio::test]
async fn test_asking_for_more_screens_than_the_image_has_is_refused() {
    let (status, body) = send(post("/v1/boxes", r#"{"spec":{"desktop":{"screens":99}}}"#)).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "bad_request");
}

#[tokio::test]
async fn test_a_misspelled_key_is_refused_in_the_same_shape_as_everything_else() {
    let (status, body) = send(post("/v1/boxes", r#"{"spec":{"desktop":{"widht":800}}}"#)).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "bad_request");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("widht"),
        "the refusal names the key that was not understood: {body}"
    );
}

#[tokio::test]
async fn test_a_body_that_is_not_json_answers_json_anyway() {
    let (status, body) = send(post("/v1/boxes", "not json at all")).await;

    // A client parses errors one way or it parses them twice. This is the
    // first error most clients will ever see.
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "bad_request");
}

#[tokio::test]
async fn test_driving_a_box_that_is_not_here_is_not_found() {
    let (status, body) = send(post(
        "/v1/boxes/box_nope/screens/0/actions",
        r#"{"actions":[{"type":"key","chord":"ctrl+a"}]}"#,
    ))
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
}

#[tokio::test]
async fn test_an_action_this_api_does_not_have_is_refused() {
    let (status, _) = send(post(
        "/v1/boxes/box_nope/screens/0/actions",
        r#"{"actions":[{"type":"teleport","to":{"x":1,"y":2}}]}"#,
    ))
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_an_empty_command_is_refused_rather_than_run() {
    let (status, _) = send(post("/v1/boxes/box_nope/exec", r#"{"argv":[]}"#)).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_a_box_that_did_nothing_has_no_trace() {
    let (status, body) = send(get("/v1/boxes/box_nope/trace")).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
}

#[tokio::test]
async fn test_a_frame_from_a_trace_that_does_not_exist_is_not_found() {
    let (status, _) = send(get("/v1/boxes/box_nope/trace/frames/abc123")).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_forking_a_box_nobody_ever_traced_is_not_found() {
    let (status, body) = send(post("/v1/boxes/box_nope/fork", "{}")).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
}

#[tokio::test]
async fn test_a_snapshot_fork_says_why_it_cannot() {
    let (status, body) = send(post("/v1/boxes/box_nope/fork", r#"{"mode":"snapshot"}"#)).await;

    // Refused ahead of the missing box: the caller needs to hear that no
    // substrate here can do this at all, not that this one box is absent.
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(body["code"], "unsupported");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("replay"),
        "the refusal names what to use instead: {body}"
    );
}

/// A server with a token turns away everything but health.
async fn send_gated(request: Request<Body>) -> (StatusCode, Value) {
    let state = Arc::new(AppState {
        token: Some(computer::Secret::new("0123456789abcdef0123").expect("a secret")),
        ..AppState::default()
    });
    let response = routes::router(state)
        .oneshot(request)
        .await
        .expect("the router answered");

    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };

    (status, body)
}

fn with_token(path: &str, token: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("a request")
}

#[tokio::test]
async fn test_a_gated_api_refuses_a_request_carrying_nothing() {
    let (status, body) = send_gated(get("/v1/boxes")).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "denied");
}

#[tokio::test]
async fn test_a_gated_api_refuses_the_wrong_token() {
    let (status, _) = send_gated(with_token("/v1/boxes", "0123456789abcdef0124")).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_the_right_token_gets_through() {
    let (status, body) = send_gated(with_token("/v1/boxes", "0123456789abcdef0123")).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["boxes"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn test_health_answers_without_one() {
    // A load balancer has no token, and a refusal would tell whoever asked the
    // same thing this does.
    let (status, body) = send_gated(get("/v1/health")).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn test_an_ungated_api_on_loopback_still_opens() {
    let (status, _) = send(get("/v1/boxes")).await;

    assert_eq!(status, StatusCode::OK);
}
