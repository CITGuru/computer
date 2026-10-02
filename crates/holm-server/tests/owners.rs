use axum::body::Body;
use axum::extract::Query;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use holm::sandboxes::remote::RemoteApi;
use holm::testing::ScriptedRemote;
use holm::{Secret, testing::ScriptedEngine};
use holm_server::caller::ConsoleKey;
use holm_server::console::Console;
use holm_server::runtimes::{self, Runtimes, Vendors};
use holm_server::secrets::Keeper;
use holm_server::{AppState, routes};
use http_body_util::BodyExt;
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use tower::ServiceExt;

const OPERATOR: &str = "operator_token_0123456789";

struct Scripted(Arc<ScriptedRemote>);

impl Vendors for Scripted {
    fn build(
        &self,
        _provider: &str,
        _fields: &Value,
        _key: Option<&Secret>,
    ) -> Result<Arc<dyn RemoteApi>, String> {
        Ok(Arc::clone(&self.0) as Arc<dyn RemoteApi>)
    }

    fn in_environment(&self, _provider: &str) -> Option<Arc<dyn RemoteApi>> {
        None
    }
}

struct World {
    state: Arc<AppState>,
    console: Ed25519KeyPair,
    remote: Arc<ScriptedRemote>,
}

impl World {
    async fn new() -> Self {
        Self::made(None).await
    }

    async fn made(console_at: Option<&str>) -> Self {
        let document = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).expect("a key");
        let console = Ed25519KeyPair::from_pkcs8(document.as_ref()).expect("a pair");
        let key = ConsoleKey::parse(&STANDARD.encode(console.public_key().as_ref()))
            .expect("a public key");

        let runtimes = Runtimes::default();
        runtimes.add(runtimes::engine(
            "docker",
            Arc::new(holm::EngineMachine::new(Arc::new(ScriptedEngine::new()))),
        ));
        runtimes.settle();

        let remote = Arc::new(ScriptedRemote::new().building());
        let state = AppState::default()
            .with(runtimes)
            .keeping(Keeper::of(&[7u8; 32]).expect("a key"))
            .serving(Arc::new(Scripted(Arc::clone(&remote))))
            .gated(Some(Secret::new(OPERATOR).expect("a token")));
        let state = Arc::new(match console_at {
            Some(base) => state.linking(Console::linked(base, None, Some(key)).expect("a console")),
            None => state.trusting(key),
        });
        let world = Self {
            state,
            console,
            remote,
        };

        let (status, _) = world
            .send(
                OPERATOR,
                "POST",
                "/v1/runtimes",
                Some(json!({
                    "name": "cloud",
                    "provider": "scripted",
                    "secrets": { "api_key": "vendor_key_0123456789" }
                })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "the operator shares a runtime");
        world
    }

    fn token(&self, workspace: &str, role: &str) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"EdDSA","typ":"JWT"}"#);
        let claims = json!({
            "iss": "console", "aud": "holmd", "sub": "usr_1",
            "ws": workspace, "role": role, "exp": u64::MAX / 2,
        });
        let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
        let signature = self.console.sign(format!("{header}.{payload}").as_bytes());
        format!(
            "{header}.{payload}.{}",
            URL_SAFE_NO_PAD.encode(signature.as_ref())
        )
    }

    async fn send(
        &self,
        bearer: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        self.send_with(bearer, method, path, body, &[]).await
    }

    async fn send_with(
        &self,
        bearer: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(path);
        if !bearer.is_empty() {
            request = request.header("authorization", format!("Bearer {bearer}"));
        }
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let request = match body {
            Some(body) => request
                .header("content-type", "application/json")
                .body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        }
        .expect("a request");

        let response = routes::router(Arc::clone(&self.state))
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
        let body = match bytes.is_empty() {
            true => Value::Null,
            false => serde_json::from_slice(&bytes).expect("json"),
        };
        (status, body)
    }

    async fn launch(&self, bearer: &str) -> String {
        let (status, body) = self
            .send(
                bearer,
                "POST",
                "/v1/boxes",
                Some(json!({ "placement": { "runtime": "cloud" } })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body["id"].as_str().expect("an id").to_string()
    }
}

fn ids(body: &Value) -> Vec<String> {
    body["boxes"]
        .as_array()
        .expect("boxes")
        .iter()
        .map(|one| one["id"].as_str().expect("an id").to_string())
        .collect()
}

#[tokio::test]
async fn test_a_workspace_sees_and_drives_only_its_own_boxes() {
    let world = World::new().await;
    let (a, b) = (world.token("ws_a", "member"), world.token("ws_b", "member"));

    let id = world.launch(&a).await;

    let (_, mine) = world.send(&a, "GET", "/v1/boxes", None).await;
    assert_eq!(ids(&mine), std::slice::from_ref(&id));
    let (_, theirs) = world.send(&b, "GET", "/v1/boxes", None).await;
    assert!(
        ids(&theirs).is_empty(),
        "another workspace's box is not listed"
    );

    for (method, path) in [
        ("GET", format!("/v1/boxes/{id}")),
        ("GET", format!("/v1/boxes/{id}/trace")),
        ("POST", format!("/v1/boxes/{id}/screens/0/viewer/ticket")),
    ] {
        let (status, _) = world.send(&b, method, &path, None).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{method} {path} answers as if the box were not there"
        );
    }
    let (status, _) = world
        .send_with(
            &b,
            "DELETE",
            &format!("/v1/boxes/{id}"),
            None,
            &[("x-holm-confirm-delete", "true")],
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "nor can it remove it");

    let (_, everything) = world.send(OPERATOR, "GET", "/v1/boxes", None).await;
    assert_eq!(
        ids(&everything),
        std::slice::from_ref(&id),
        "the operator sees every workspace"
    );
    assert_eq!(everything["boxes"][0]["owner"], "ws_a");
}

#[tokio::test]
async fn test_a_box_view_carries_its_spec_and_placement() {
    let world = World::new().await;
    let a = world.token("ws_a", "member");
    let id = world.launch(&a).await;

    let (status, body) = world
        .send(&a, "GET", &format!("/v1/boxes/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["placement"]["runtime"], "cloud");
    assert!(
        body["spec"].is_object(),
        "so the box can be launched again from it"
    );
}

#[tokio::test]
async fn test_a_viewer_watches_but_cannot_change_or_drive() {
    let world = World::new().await;
    let id = world.launch(&world.token("ws_a", "member")).await;
    let viewer = world.token("ws_a", "viewer");

    let (status, _) = world
        .send(&viewer, "GET", &format!("/v1/boxes/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = world
        .send(
            &viewer,
            "POST",
            "/v1/boxes",
            Some(json!({ "placement": { "runtime": "cloud" } })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a viewer launches nothing");

    let (status, body) = world
        .send(
            &viewer,
            "POST",
            &format!("/v1/boxes/{id}/screens/0/viewer/ticket"),
            None,
        )
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a viewer may open the screen to watch"
    );
    let ticket = body["ticket"].as_str().expect("a ticket");
    assert!(
        !world.state.tickets.controls(ticket),
        "but its ticket does not open the control socket"
    );
}

#[tokio::test]
async fn test_a_workspace_runtime_is_its_own_and_a_shared_one_is_the_operators_to_change() {
    let world = World::new().await;
    let (admin, member, other) = (
        world.token("ws_a", "admin"),
        world.token("ws_a", "member"),
        world.token("ws_b", "admin"),
    );

    let (status, _) = world
        .send(&member, "POST", "/v1/runtimes", Some(json!({
            "name": "a-cloud", "provider": "scripted", "secrets": { "api_key": "vendor_key_0123456789" }
        })))
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a member does not add runtimes"
    );

    let (status, body) = world
        .send(&admin, "POST", "/v1/runtimes", Some(json!({
            "name": "a-cloud", "provider": "scripted", "secrets": { "api_key": "vendor_key_0123456789" }
        })))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["owner"], "ws_a");

    let names = |body: &Value| -> Vec<String> {
        body["runtimes"]
            .as_array()
            .expect("runtimes")
            .iter()
            .map(|one| one["name"].as_str().expect("a name").to_string())
            .collect()
    };
    let (_, seen) = world.send(&other, "GET", "/v1/runtimes", None).await;
    assert!(
        !names(&seen).contains(&"a-cloud".to_string()),
        "another workspace's runtime is not listed"
    );
    assert!(
        names(&seen).contains(&"cloud".to_string()),
        "a shared one is"
    );

    let (status, _) = world
        .send(
            &other,
            "POST",
            "/v1/boxes",
            Some(json!({ "placement": { "runtime": "a-cloud" } })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "nor can it launch on it");

    let (status, _) = world
        .send(
            &admin,
            "PATCH",
            "/v1/runtimes/cloud",
            Some(json!({ "secrets": {} })),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a shared runtime is changed by the operator only"
    );
}

#[tokio::test]
async fn test_an_unsigned_caller_is_refused_once_a_console_is_trusted() {
    let world = World::new().await;

    let (status, _) = world.send("", "GET", "/v1/boxes", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = world.send("not-a-token", "GET", "/v1/boxes", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_one_idempotency_key_in_two_workspaces_is_two_boxes() {
    let world = World::new().await;
    let body = json!({ "placement": { "runtime": "cloud" } });
    let key = [("idempotency-key", "same-key-0001")];

    let (_, first) = world
        .send_with(
            &world.token("ws_a", "member"),
            "POST",
            "/v1/boxes",
            Some(body.clone()),
            &key,
        )
        .await;
    let (_, second) = world
        .send_with(
            &world.token("ws_b", "member"),
            "POST",
            "/v1/boxes",
            Some(body),
            &key,
        )
        .await;

    assert_ne!(
        first["id"], second["id"],
        "a key is a workspace's own, never a way to read another's reply"
    );
}

#[tokio::test]
async fn test_a_vendor_box_gets_its_own_viewer_key_and_tickets_outlive_the_process() {
    let world = World::new().await;
    let a = world.token("ws_a", "member");
    let id = world.launch(&a).await;

    let started = world.remote.plans().pop().expect("a launch").env;
    assert_eq!(
        started.get(holm::AUTH_ENV).map(String::as_str),
        Some("signed")
    );
    assert_eq!(
        started.get(holm::VIEWER_KEY_ENV).map(String::as_str),
        world.state.doors.box_key(&id).as_ref().map(Secret::expose),
        "the key is derived from the box id, so any process with the server key can sign for it"
    );
    assert!(!started.contains_key(holm::VIEW_SECRET_ENV));

    let (status, body) = world
        .send(
            &a,
            "POST",
            &format!("/v1/boxes/{id}/screens/0/viewer/ticket"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ticket = body["ticket"].as_str().expect("a ticket");

    let (_, view) = world
        .send(&a, "GET", &format!("/v1/boxes/{id}"), None)
        .await;
    if let Some(url) = view["viewer_url"].as_str() {
        assert!(
            url.contains("token"),
            "a watch link for a signed box carries a short-lived token: {url}"
        );
    }

    let restarted = AppState::default().keeping(Keeper::of(&[7u8; 32]).expect("a key"));
    assert!(restarted.tickets.admits(ticket, &id, 0));
    let other = AppState::default().keeping(Keeper::of(&[8u8; 32]).expect("a key"));
    assert!(!other.tickets.admits(ticket, &id, 0));
}

#[tokio::test]
async fn test_a_removed_box_leaves_every_list_and_keeps_its_record() {
    let world = World::new().await;
    let id = world.launch(OPERATOR).await;
    let path = format!("/v1/boxes/{id}");
    let confirm = [("x-holm-confirm-delete", "true")];

    let other = Arc::new(
        AppState::split(
            Arc::clone(&world.state.store),
            Arc::clone(&world.state.frames),
        )
        .serving(Arc::new(Scripted(Arc::clone(&world.remote))))
        .keeping(Keeper::of(&[7u8; 32]).expect("a key")),
    );
    runtimes::from_store(
        &other.runtimes,
        other.store.as_ref(),
        &other.secrets,
        other.vendors.as_ref(),
    )
    .await;
    assert!(
        other.entry(&id).await.is_ok(),
        "a second server takes the box back from its record"
    );

    let (status, _) = world
        .send_with(OPERATOR, "DELETE", &path, None, &confirm)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, body) = world.send(OPERATOR, "GET", &path, None).await;
    assert_eq!(status, StatusCode::GONE, "{body}");
    let (_, listed) = world.send(OPERATOR, "GET", "/v1/boxes", None).await;
    assert!(ids(&listed).is_empty());

    let response = routes::router(Arc::clone(&other))
        .oneshot(
            Request::builder()
                .uri("/v1/boxes")
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("answered");
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    let listed: Value = serde_json::from_slice(&bytes).expect("json");
    assert!(
        ids(&listed).is_empty(),
        "the server that still held the box drops it when it lists: {listed}"
    );
    assert!(other.entry(&id).await.is_err(), "and does not take it back");

    let record = world
        .state
        .store
        .get_box(&id)
        .await
        .expect("asked")
        .expect("the record stays for history and usage");
    assert!(record.deleted_at_ms.is_some());

    let (status, _) = world
        .send_with(OPERATOR, "DELETE", &path, None, &confirm)
        .await;
    assert_eq!(status, StatusCode::GONE, "a second delete says it is gone");
}

#[tokio::test]
async fn test_each_change_to_a_box_is_an_event_its_workspace_can_read() {
    let world = World::new().await;
    let (a, b) = (world.token("ws_a", "member"), world.token("ws_b", "member"));
    let id = world.launch(&a).await;

    let takeover = format!("/v1/boxes/{id}/screens/0/takeover");
    let (status, body) = world
        .send(&a, "POST", &takeover, Some(json!({ "shared": false })))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = world.send(&a, "DELETE", &takeover, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = world
        .send_with(
            &a,
            "DELETE",
            &format!("/v1/boxes/{id}"),
            None,
            &[("x-holm-confirm-delete", "true")],
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    tokio::time::sleep(std::time::Duration::from_millis(
        holm_server::events::SETTLE_MS + 200,
    ))
    .await;

    let kinds = |body: &Value| -> Vec<String> {
        body["events"]
            .as_array()
            .expect("events")
            .iter()
            .filter(|event| event["box_id"] == id.as_str())
            .map(|event| event["kind"].as_str().expect("a kind").to_string())
            .collect()
    };

    let (status, mine) = world.send(&a, "GET", "/v1/events", None).await;
    assert_eq!(status, StatusCode::OK, "{mine}");
    assert_eq!(
        kinds(&mine),
        [
            "box.created",
            "box.ready",
            "screen.taken_over",
            "screen.given_back",
            "box.removed"
        ]
    );
    let first = &mine["events"][0];
    assert_eq!(first["owner"], "ws_a");
    assert_eq!(first["runtime"], "cloud");

    let (_, theirs) = world.send(&b, "GET", "/v1/events", None).await;
    assert!(
        kinds(&theirs).is_empty(),
        "another workspace reads none of them"
    );

    let (_, all) = world.send(OPERATOR, "GET", "/v1/events", None).await;
    assert_eq!(kinds(&all).len(), 5, "the operator reads every workspace");

    let after = mine["next"].as_u64().expect("a cursor");
    let (_, later) = world
        .send(&a, "GET", &format!("/v1/events?after={after}"), None)
        .await;
    assert!(
        later["events"].as_array().expect("events").is_empty(),
        "a read after the cursor gives only what is new"
    );
}

#[tokio::test]
async fn test_a_box_the_runtime_lost_is_said_once_until_it_is_ready_again() {
    use holm_api::{Actor, TraceEvent};

    let world = World::new().await;
    let id = world.launch(OPERATOR).await;
    let lost = || TraceEvent::Gone {
        why: holm_server::reap::LOST.to_string(),
    };

    world.state.record(&id, Actor::System, lost()).await;
    world.state.record(&id, Actor::System, lost()).await;
    world
        .state
        .record(&id, Actor::Agent, TraceEvent::BoxStarted)
        .await;
    world.state.record(&id, Actor::System, lost()).await;

    let kinds: Vec<String> = world
        .state
        .store
        .events_after(0, u64::MAX / 2, 50)
        .await
        .expect("read")
        .into_iter()
        .filter(|event| event.box_id.as_deref() == Some(id.as_str()))
        .map(|event| event.kind)
        .collect();
    assert_eq!(
        kinds,
        [
            "box.created",
            "box.ready",
            "box.unreachable",
            "box.ready",
            "box.unreachable"
        ],
        "a server that finds the same lost box at each start does not say it again"
    );
}

async fn console_without_credit_for(unfunded: &'static str) -> String {
    let funds = move |Query(asked): Query<BTreeMap<String, String>>| async move {
        match asked.get("workspace").map(String::as_str) == Some(unfunded) {
            true => Json(json!({ "allowed": false, "reason": "no credit left" })),
            false => Json(json!({ "allowed": true })),
        }
    };
    let app = Router::new().route("/api/holmd/funds", get(funds));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let address = listener.local_addr().expect("an address");
    tokio::spawn(async move { axum::serve(listener, app).await.expect("served") });
    format!("http://{address}")
}

#[tokio::test]
async fn test_a_shared_runtime_takes_no_box_from_a_workspace_with_no_credit() {
    let world = World::made(Some(&console_without_credit_for("ws_a").await)).await;
    let (a, b) = (world.token("ws_a", "admin"), world.token("ws_b", "member"));
    let on_cloud = Some(json!({ "placement": { "runtime": "cloud" } }));

    let (status, body) = world.send(&a, "POST", "/v1/boxes", on_cloud.clone()).await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(body["message"], "no credit left");

    world.launch(&b).await;
    let (status, _) = world.send(OPERATOR, "POST", "/v1/boxes", on_cloud).await;
    assert_eq!(status, StatusCode::CREATED, "the operator owes nobody");

    let (status, _) = world
        .send(
            &a,
            "POST",
            "/v1/runtimes",
            Some(json!({
                "name": "mine",
                "provider": "scripted",
                "secrets": { "api_key": "own_key_0123456789" }
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = world
        .send(
            &a,
            "POST",
            "/v1/boxes",
            Some(json!({ "placement": { "runtime": "mine" } })),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a runtime on the workspace's own key needs no credit: {body}"
    );
}

#[tokio::test]
async fn test_an_image_on_a_shared_runtime_is_the_operators_to_remove() {
    let world = World::new().await;
    let a = world.token("ws_a", "admin");
    let id = world.launch(&a).await;

    let (_, images) = world.send(&a, "GET", "/v1/images", None).await;
    let digest = images["images"][0]["spec_digest"]
        .as_str()
        .expect("the launch recorded an image")
        .to_string();
    let image = format!("/v1/runtimes/cloud/images/{digest}");

    let (status, _) = world
        .send_with(
            &a,
            "DELETE",
            &format!("/v1/boxes/{id}"),
            None,
            &[("x-holm-confirm-delete", "true")],
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, body) = world.send(&a, "DELETE", &image, None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "another workspace may start boxes from it: {body}"
    );
    let (status, _) = world.send(OPERATOR, "DELETE", &image, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}
