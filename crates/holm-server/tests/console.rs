use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use holm_server::caller::{Caller, Role};
use holm_server::console::{Console, Refusal, Revoked};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

const SECRET: &str = "console_secret_0123456789";
const NOW: u64 = 1_800_000_000;

#[derive(Default)]
struct Held {
    published: Vec<(String, Vec<u8>)>,
    revoked: Revoked,
    reported: Vec<Value>,
    unfunded: Vec<String>,
    down: bool,
}

type Shared = Arc<Mutex<Held>>;

async fn keys(State(held): State<Shared>) -> Result<Json<Value>, StatusCode> {
    let held = held.lock().expect("held");
    if held.down {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let keys: Vec<Value> = held
        .published
        .iter()
        .map(|(kid, public)| json!({ "kty": "OKP", "crv": "Ed25519", "kid": kid, "x": URL_SAFE_NO_PAD.encode(public) }))
        .collect();
    Ok(Json(json!({ "keys": keys })))
}

fn authorized(headers: &HeaderMap) -> bool {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        == Some(&format!("Bearer {SECRET}"))
}

async fn revoked(
    State(held): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Revoked>, StatusCode> {
    let held = held.lock().expect("held");
    match (held.down, authorized(&headers)) {
        (true, _) => Err(StatusCode::SERVICE_UNAVAILABLE),
        (_, false) => Err(StatusCode::UNAUTHORIZED),
        _ => Ok(Json(held.revoked.clone())),
    }
}

async fn usage(
    State(held): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> StatusCode {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED;
    }
    held.lock().expect("held").reported.push(body);
    StatusCode::NO_CONTENT
}

async fn funds(
    State(held): State<Shared>,
    headers: HeaderMap,
    Query(asked): Query<BTreeMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let held = held.lock().expect("held");
    let workspace = asked.get("workspace").cloned().unwrap_or_default();
    match (held.down, authorized(&headers)) {
        (true, _) => Err(StatusCode::SERVICE_UNAVAILABLE),
        (_, false) => Err(StatusCode::UNAUTHORIZED),
        _ if held.unfunded.contains(&workspace) => Ok(Json(
            json!({ "allowed": false, "reason": "no credit left" }),
        )),
        _ => Ok(Json(json!({ "allowed": true }))),
    }
}

async fn fake_console() -> (String, Shared) {
    let held: Shared = Arc::default();
    let app = Router::new()
        .route("/api/holmd/keys.json", get(keys))
        .route("/api/holmd/revoked", get(revoked))
        .route("/api/holmd/usage", post(usage))
        .route("/api/holmd/funds", get(funds))
        .with_state(Arc::clone(&held));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let address = listener.local_addr().expect("an address");
    tokio::spawn(async move { axum::serve(listener, app).await.expect("served") });
    (format!("http://{address}"), held)
}

fn pair() -> Ed25519KeyPair {
    let document = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).expect("a key");
    Ed25519KeyPair::from_pkcs8(document.as_ref()).expect("a pair")
}

fn signed(pair: &Ed25519KeyPair, kid: &str, claims: Value) -> String {
    let header =
        URL_SAFE_NO_PAD.encode(json!({ "alg": "EdDSA", "typ": "JWT", "kid": kid }).to_string());
    let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
    let signature = pair.sign(format!("{header}.{payload}").as_bytes());
    format!(
        "{header}.{payload}.{}",
        URL_SAFE_NO_PAD.encode(signature.as_ref())
    )
}

fn api_key(pair: &Ed25519KeyPair, kid: &str, id: &str, workspace: &str) -> String {
    let claims = json!({ "iss": "console", "aud": "holmd", "kind": "key", "sub": id, "ws": workspace, "role": "member" });
    format!("holm_sk_{}", signed(pair, kid, claims))
}

fn call(pair: &Ed25519KeyPair, kid: &str) -> String {
    signed(
        pair,
        kid,
        json!({ "iss": "console", "aud": "holmd", "ws": "ws_a", "role": "admin", "exp": NOW + 60 }),
    )
}

#[tokio::test]
async fn test_an_api_key_is_checked_against_the_consoles_published_keys_and_revocations() {
    let (base, held) = fake_console().await;
    let (old, new) = (pair(), pair());
    held.lock().expect("held").published = vec![
        ("k1".into(), old.public_key().as_ref().to_vec()),
        ("k2".into(), new.public_key().as_ref().to_vec()),
    ];
    let console = Console::linked(&base, Some(SECRET.into()), None).expect("a console");
    let member = Some(Caller::of("ws_a", Role::Member));

    let key = api_key(&new, "k2", "key_1", "ws_a");
    assert_eq!(
        console.verify(&key, NOW).await,
        None,
        "no key is taken before holmd knows which are revoked"
    );

    console.refresh_revoked().await.expect("the revoked list");
    assert_eq!(
        console.verify(&key, NOW).await,
        member,
        "an unknown kid makes it fetch the published keys"
    );
    assert_eq!(
        console
            .verify(&api_key(&old, "k1", "key_2", "ws_a"), NOW)
            .await,
        member,
        "an older key still published still verifies, so the console can rotate"
    );
    assert_eq!(
        console.verify(&call(&new, "k2"), NOW).await,
        Some(Caller::of("ws_a", Role::Admin)),
        "the same keys check the console's call tokens"
    );

    held.lock()
        .expect("held")
        .revoked
        .keys
        .insert("key_1".into());
    console.refresh_revoked().await.expect("the revoked list");
    assert_eq!(
        console.verify(&key, NOW).await,
        None,
        "a revoked key is refused"
    );

    held.lock()
        .expect("held")
        .revoked
        .workspaces
        .insert("ws_a".into());
    console.refresh_revoked().await.expect("the revoked list");
    assert_eq!(
        console
            .verify(&api_key(&new, "k2", "key_3", "ws_a"), NOW)
            .await,
        None,
        "every key of a deleted workspace is refused"
    );
}

#[tokio::test]
async fn test_a_console_that_goes_away_leaves_what_holmd_already_knows() {
    let (base, held) = fake_console().await;
    let signer = pair();
    held.lock().expect("held").published =
        vec![("k1".into(), signer.public_key().as_ref().to_vec())];
    let console = Console::linked(&base, Some(SECRET.into()), None).expect("a console");
    console.refresh_keys().await.expect("the keys");
    console.refresh_revoked().await.expect("the revoked list");

    held.lock().expect("held").down = true;
    assert!(console.refresh_revoked().await.is_err());
    assert!(
        console
            .verify(&api_key(&signer, "k1", "key_1", "ws_a"), NOW)
            .await
            .is_some(),
        "the last list still holds"
    );
    assert_eq!(
        console
            .verify(&api_key(&pair(), "k9", "key_2", "ws_a"), NOW)
            .await,
        None,
        "a key from a signer it never saw is refused"
    );
}

#[tokio::test]
async fn test_key_use_is_reported_to_the_console_with_its_secret() {
    let (base, held) = fake_console().await;
    let signer = pair();
    held.lock().expect("held").published =
        vec![("k1".into(), signer.public_key().as_ref().to_vec())];
    let console = Console::linked(&base, Some(SECRET.into()), None).expect("a console");
    console.refresh_revoked().await.expect("the revoked list");

    assert!(
        console
            .verify(&api_key(&signer, "k1", "key_1", "ws_a"), NOW)
            .await
            .is_some()
    );
    console.report_usage().await.expect("reported");

    let reported = held.lock().expect("held").reported.clone();
    let keys: BTreeMap<String, u64> =
        serde_json::from_value(reported[0]["keys"].clone()).expect("keys");
    assert_eq!(keys.get("key_1"), Some(&(NOW * 1000)));

    console.report_usage().await.expect("nothing new");
    assert_eq!(
        held.lock().expect("held").reported.len(),
        1,
        "a report is sent only when a key was used"
    );
}

fn push_request(secret: &str, body: &str, at: u64) -> axum::http::Request<axum::body::Body> {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
    let tag = ring::hmac::sign(&key, format!("msg_1.{at}.{body}").as_bytes());
    let signature = format!(
        "v1,{}",
        base64::engine::general_purpose::STANDARD.encode(tag.as_ref())
    );

    axum::http::Request::builder()
        .method("POST")
        .uri(holm_server::console::PUSH_PATH)
        .header(holm_server::console::PUSH_ID, "msg_1")
        .header(holm_server::console::PUSH_TIMESTAMP, at.to_string())
        .header(holm_server::console::PUSH_SIGNATURE, signature)
        .body(axum::body::Body::from(body.to_string()))
        .expect("a request")
}

#[tokio::test]
async fn test_a_pushed_revocation_stops_a_key_on_every_server_that_shares_the_store() {
    use holm_server::{AppState, routes};
    use tower::ServiceExt;

    let (base, held) = fake_console().await;
    let signer = pair();
    held.lock().expect("held").published =
        vec![("k1".into(), signer.public_key().as_ref().to_vec())];

    let linked = || Console::linked(&base, Some(SECRET.into()), None).expect("a console");
    let state = Arc::new(AppState::default().linking(linked()));
    let console = state.console.clone().expect("a console");
    console.refresh_revoked().await.expect("the revoked list");

    let other =
        AppState::split(Arc::clone(&state.store), Arc::clone(&state.frames)).linking(linked());
    let other = other.console.clone().expect("a console");
    other.refresh_revoked().await.expect("the revoked list");

    let key = api_key(&signer, "k1", "key_1", "ws_a");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock")
        .as_secs();
    assert!(console.verify(&key, now).await.is_some());

    let body = r#"{"keys":["key_1"],"workspaces":[]}"#;
    let send = |request: axum::http::Request<axum::body::Body>| {
        let state = Arc::clone(&state);
        async move {
            routes::router(state)
                .oneshot(request)
                .await
                .expect("answered")
                .status()
        }
    };

    assert_eq!(
        send(push_request("another secret entirely", body, now)).await,
        StatusCode::UNAUTHORIZED,
        "a push the console's secret did not sign is refused"
    );
    assert_eq!(
        send(push_request(SECRET, body, now - 3600)).await,
        StatusCode::UNAUTHORIZED,
        "and so is an old one"
    );
    assert!(console.verify(&key, now).await.is_some());

    assert_eq!(
        send(push_request(SECRET, body, now)).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        console.verify(&key, now).await,
        None,
        "the key stops at once, with no wait for the next pull"
    );
    assert_eq!(
        other.verify(&key, now).await,
        None,
        "and on a server that never got the push, because it reads the same store"
    );
    assert!(
        console
            .verify(&api_key(&signer, "k1", "key_2", "ws_a"), now)
            .await
            .is_some(),
        "another key of the workspace still works"
    );
}

#[tokio::test]
async fn test_a_console_that_still_serves_the_earlier_paths_is_read_there() {
    let held: Shared = Arc::default();
    let app = Router::new()
        .route("/api/computerd/keys.json", get(keys))
        .route("/api/computerd/revoked", get(revoked))
        .route("/api/computerd/usage", post(usage))
        .route("/api/computerd/funds", get(funds))
        .fallback(|| async { axum::response::Html("<html>sign in</html>") })
        .with_state(Arc::clone(&held));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let base = format!("http://{}", listener.local_addr().expect("an address"));
    tokio::spawn(async move { axum::serve(listener, app).await.expect("served") });

    let signer = pair();
    held.lock().expect("held").published =
        vec![("k1".into(), signer.public_key().as_ref().to_vec())];
    let console = Console::linked(&base, Some(SECRET.into()), None).expect("a console");

    console.refresh_revoked().await.expect("the revoked list");
    assert!(
        console
            .verify(&api_key(&signer, "k1", "key_1", "ws_a"), NOW)
            .await
            .is_some(),
        "a console from before the rename still gives its keys"
    );
    console.report_usage().await.expect("reported");
    assert_eq!(held.lock().expect("held").reported.len(), 1);
    assert_eq!(console.funds("ws_a").await, Ok(()));
}

#[tokio::test]
async fn test_a_workspace_with_no_credit_is_refused_and_a_silent_console_keeps_its_last_answer() {
    let (base, held) = fake_console().await;
    held.lock().expect("held").unfunded = vec!["ws_b".into()];
    let console = Console::linked(&base, Some(SECRET.into()), None).expect("a console");

    assert_eq!(console.funds("ws_a").await, Ok(()));
    assert_eq!(
        console.funds("ws_b").await,
        Err(Refusal::Unfunded("no credit left".into())),
        "the console's reason is what the caller reads"
    );

    held.lock().expect("held").unfunded.clear();
    assert_eq!(
        console.funds("ws_b").await,
        Ok(()),
        "a refusal is not kept, so bought credit counts at once"
    );

    held.lock().expect("held").down = true;
    assert_eq!(
        console.funds("ws_a").await,
        Ok(()),
        "a workspace that had credit keeps working while the console is away"
    );
    assert!(
        matches!(console.funds("ws_c").await, Err(Refusal::Unknown(_))),
        "one nothing is known about is not let onto a shared runtime"
    );

    let pinned = Console::pinned(
        holm_server::caller::ConsoleKey::raw(pair().public_key().as_ref().to_vec()).expect("a key"),
    );
    assert_eq!(
        pinned.funds("ws_b").await,
        Ok(()),
        "with no console to ask there is no credit to check"
    );
}
