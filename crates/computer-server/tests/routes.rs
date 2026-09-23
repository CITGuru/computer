use axum::body::Body;
use axum::http::{Request, StatusCode};
use computer::testing::ScriptedCli;
use computer_server::{AppState, routes};
use http_body_util::BodyExt;
use serde_json::Value;
use std::sync::Arc;
use tower::ServiceExt;

fn nowhere() -> Arc<AppState> {
    Arc::new(AppState::default().through(Some(Arc::new(ScriptedCli::new()))))
}

async fn send(request: Request<Body>) -> (StatusCode, Value) {
    let router = routes::router(nowhere());
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

    // The guard comes before the lookup, or a caller never learns about the header.
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
async fn test_a_spec_naming_an_unknown_app_is_refused_before_anything_starts() {
    let (status, body) = send(post("/v1/boxes", r#"{"spec":{"apps":{"gimpp":{}}}}"#)).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("gimpp"),
        "the refusal names the app: {body}"
    );
}

#[tokio::test]
async fn test_a_spec_naming_its_own_apt_source_is_refused_by_default() {
    let spec = r#"{"spec":{"apps":{"thing":{"packages":["thing"],
        "source":{"key_url":"https://example.invalid/k.asc",
                  "list":"https://example.invalid/r stable main"}}}}}"#;
    let (status, body) = send(post("/v1/boxes", spec)).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("custom_sources"),
        "the refusal says what would allow it: {body}"
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

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "bad_request");
}

#[tokio::test]
async fn test_driving_a_box_that_is_not_here_is_not_found() {
    let (status, body) = send(post(
        "/v1/boxes/box_nope/screens/0/actions",
        r#"{"actions":[{"type":"press","chord":"ctrl+a"}]}"#,
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

async fn send_gated(request: Request<Body>) -> (StatusCode, Value) {
    let state = Arc::new(AppState::default().gated(Some(
        computer::Secret::new("0123456789abcdef0123").expect("a secret"),
    )));
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
    let (status, body) = send_gated(get("/v1/health")).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn test_an_ungated_api_on_loopback_still_opens() {
    let (status, _) = send(get("/v1/boxes")).await;

    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_an_accepted_spec_is_built_through_the_runtime_it_was_given() {
    let cli = Arc::new(ScriptedCli::new());
    let state = Arc::new(
        AppState::default().through(Some(Arc::clone(&cli) as Arc<dyn computer::ContainerCli>)),
    );

    let response = routes::router(state)
        .oneshot(post("/v1/boxes", r#"{"spec":{"apps":{"vscode":{}}}}"#))
        .await
        .expect("the router answered");

    assert_eq!(response.status(), StatusCode::CREATED);
    assert!(
        cli.count() > 0,
        "the box was built somewhere, and it has to have been here"
    );
    assert!(
        cli.calls()
            .iter()
            .any(|call| call.first().is_some_and(|verb| verb == "run")),
        "a container was started through the double: {:?}",
        cli.calls()
    );
}

#[tokio::test]
async fn test_the_catalog_names_what_a_launch_can_ask_for() {
    let (status, body) = send(get("/v1/catalog")).await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        body.get("vscode").is_some(),
        "an agent reads the names here rather than guessing one: {body}"
    );
}

#[tokio::test]
async fn test_launching_an_app_no_catalog_holds_is_refused() {
    let (status, body) = send(post(
        "/v1/boxes/box_nothing/screens/0/actions",
        r#"{"actions":[{"type":"launch","app":"gimpp"}]}"#,
    ))
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

#[tokio::test]
async fn test_the_windows_that_are_not_ids_are_routes_of_their_own() {
    // `active` and `wait` must not match as window ids.
    for request in [
        get("/v1/boxes/box_nope/screens/0/windows/active"),
        post(
            "/v1/boxes/box_nope/screens/0/windows/wait",
            r#"{"class":"xterm"}"#,
        ),
    ] {
        let (status, body) = send(request).await;

        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert_eq!(body["code"], "not_found", "the handler answered: {body}");
    }
}

#[tokio::test]
async fn test_an_arrangement_this_api_does_not_have_is_refused() {
    let (status, _) = send(post(
        "/v1/boxes/box_nope/screens/0/windows/42/arrange",
        r#"{"how":"teleport"}"#,
    ))
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_half_a_rectangle_is_refused_rather_than_guessed_at() {
    let (status, body) = send(get(
        "/v1/boxes/box_nope/screens/0/frame?x=10&y=20&width=400",
    ))
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("together"),
        "{body}"
    );
}

#[tokio::test]
async fn test_a_capture_is_of_a_window_or_a_region_and_not_both() {
    let (status, _) = send(get(
        "/v1/boxes/box_nope/screens/0/frame?window=42&x=1&y=2&width=3&height=4",
    ))
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_a_plain_frame_still_asks_for_nothing() {
    let (status, body) = send(get("/v1/boxes/box_nope/screens/0/frame")).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
}

#[tokio::test]
async fn test_a_viewer_ticket_is_for_a_box_that_is_here() {
    let (status, body) = send(post("/v1/boxes/box_nope/screens/0/viewer/ticket", "")).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
}

#[tokio::test]
async fn test_the_viewer_socket_takes_a_ticket_and_no_bearer() {
    let state = Arc::new(AppState::default().gated(Some(
        computer::Secret::new("0123456789abcdef0123").expect("a secret"),
    )));
    let request = Request::builder()
        .uri("/v1/boxes/box_nope/screens/0/viewer/socket?ticket=nothing")
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .body(Body::empty())
        .expect("a request");
    let response = routes::router(state)
        .oneshot(request)
        .await
        .expect("the router answered");

    // Without a live connection nothing can be upgraded, which is as far as a test gets;
    // what matters is that the gate did not answer first.
    assert_eq!(
        response.status(),
        StatusCode::UPGRADE_REQUIRED,
        "the socket route sits outside the bearer gate"
    );
}

#[tokio::test]
async fn test_a_cdp_token_is_for_a_box_that_is_here() {
    let (status, body) = send(post("/v1/boxes/box_nope/cdp", "")).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
}

#[tokio::test]
async fn test_a_cdp_token_is_minted_behind_the_bearer_and_used_without_one() {
    let gated = || {
        Arc::new(AppState::default().gated(Some(
            computer::Secret::new("0123456789abcdef0123").expect("a secret"),
        )))
    };

    let minted = routes::router(gated())
        .oneshot(post("/v1/boxes/box_nope/cdp", ""))
        .await
        .expect("the router answered");
    assert_eq!(
        minted.status(),
        StatusCode::UNAUTHORIZED,
        "whoever mints one reads every cookie the browser holds"
    );

    let relayed = routes::router(gated())
        .oneshot(get("/v1/cdp/nothing/json/version"))
        .await
        .expect("the router answered");
    assert_eq!(
        relayed.status(),
        StatusCode::FORBIDDEN,
        "a library that speaks CDP sends no bearer, so the short-lived token in the path is the gate"
    );
}

async fn ask_mcp(body: &str) -> (StatusCode, Value) {
    let router = computer_server::mcp::router(nowhere(), "http://127.0.0.1:1".to_string(), None);
    let response = router
        .oneshot(post("/mcp", body))
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
        serde_json::from_slice(&bytes).expect("JSON")
    };

    (status, body)
}

#[tokio::test]
async fn test_mcp_answers_over_http_with_the_page_among_its_resources() {
    let (status, body) = ask_mcp(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], 1);
    assert!(body["result"]["capabilities"]["resources"].is_object());

    let (status, body) = ask_mcp(r#"{"jsonrpc":"2.0","id":2,"method":"resources/list"}"#).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["result"]["resources"][0]["uri"],
        "ui://computer/screen.html"
    );
}

#[tokio::test]
async fn test_mcp_takes_a_batch_and_answers_a_notification_with_nothing() {
    let (status, body) = ask_mcp(
        r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/initialized"}]"#,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().map(Vec::len), Some(1), "{body}");

    let (status, body) = ask_mcp(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).await;

    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body, Value::Null);
}

#[tokio::test]
async fn test_mcp_refuses_what_is_not_json() {
    let (status, body) = ask_mcp("not json").await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], -32700);
}

#[tokio::test]
async fn test_a_server_says_which_runtimes_it_has_and_what_they_run() {
    let (status, body) = send(get("/v1/runtimes")).await;

    assert_eq!(status, StatusCode::OK);

    let runtimes = body["runtimes"].as_array().expect("a list");
    let docker = runtimes
        .iter()
        .find(|runtime| runtime["name"] == "docker")
        .expect("the engine this server was given");

    assert_eq!(docker["place"], "host");
    assert_eq!(docker["environment"]["kind"], "container");
    assert_eq!(docker["state"], "ready");
    assert_eq!(docker["boxes"], 0);
    assert_eq!(docker["can"]["reach"], "host_port");
}

#[tokio::test]
async fn test_a_runtime_this_server_does_not_have_is_not_found() {
    let (status, body) = send(get("/v1/runtimes/nowhere")).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
}

#[tokio::test]
async fn test_a_placement_naming_a_program_no_longer_runs_it() {
    let (status, body) = send(post(
        "/v1/boxes",
        r#"{"placement":{"runtime":"/usr/bin/id"}}"#,
    ))
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a runtime is a name this server offers, not a program it runs: {body}"
    );
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|why| why.contains("docker")),
        "and the refusal says what can be asked for instead: {body}"
    );
}

#[tokio::test]
async fn test_a_box_says_which_runtime_it_is_on() {
    let (status, body) = send(post("/v1/boxes", r#"{}"#)).await;

    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        body["runtime"], "docker",
        "so a restart knows where to look for it: {body}"
    );
}

#[tokio::test]
async fn test_a_box_that_asks_for_no_deadline_is_given_one() {
    let (status, body) = send(post("/v1/boxes", r#"{}"#)).await;

    assert_eq!(status, StatusCode::CREATED);

    let at = body["expires_at_ms"]
        .as_u64()
        .expect("a box with no deadline used to run until something removed it");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock")
        .as_millis() as u64;

    assert!(
        at > now && at - now <= 60 * 60 * 1000,
        "the runtime's own life is an hour: {at} against {now}"
    );
}

#[tokio::test]
async fn test_a_deadline_longer_than_the_runtime_keeps_a_box_is_refused() {
    let (status, body) = send(post(
        "/v1/boxes",
        r#"{"placement":{"expires_after_secs":86400}}"#,
    ))
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|why| why.contains("max_lifetime")),
        "the refusal says what to raise: {body}"
    );
}

#[tokio::test]
async fn test_the_deadline_a_box_was_given_is_recorded() {
    let state = nowhere();
    let response = routes::router(Arc::clone(&state))
        .oneshot(post("/v1/boxes", r#"{}"#))
        .await
        .expect("the router answered");

    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    let body: Value = serde_json::from_slice(&bytes).expect("json");
    let id = body["id"].as_str().expect("an id");

    let record = state
        .store
        .get_box(id)
        .await
        .expect("asked")
        .expect("a record");

    assert_eq!(
        record.expires_at_ms,
        body["expires_at_ms"].as_u64(),
        "a restart reads the record, so a deadline kept only in this process is lost"
    );
    assert!(record.expires_at_ms.is_some());
}
