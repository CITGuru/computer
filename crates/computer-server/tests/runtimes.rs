use axum::body::Body;
use axum::http::{Request, StatusCode};
use computer::sandboxes::remote::RemoteApi;
use computer::testing::{ScriptedEngine, ScriptedRemote};
use computer::{EngineMachine, Secret};
use computer_server::runtimes::{self, Runtimes, Vendors};
use computer_server::secrets::Keeper;
use computer_server::{AppState, routes};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

struct Scripted;

impl Vendors for Scripted {
    fn build(
        &self,
        provider: &str,
        _fields: &Value,
        key: Option<&Secret>,
    ) -> Result<Arc<dyn RemoteApi>, String> {
        if provider != "scripted" {
            return Err(format!(
                "{provider} is not a vendor this server was built with"
            ));
        }

        key.ok_or_else(|| "scripted takes an api_key".to_string())?;
        Ok(Arc::new(ScriptedRemote::new()))
    }

    fn in_environment(&self, _provider: &str) -> Option<Arc<dyn RemoteApi>> {
        None
    }
}

fn server() -> Arc<AppState> {
    let runtimes = Runtimes::default();
    runtimes.add(runtimes::engine(
        "docker",
        Arc::new(EngineMachine::new(Arc::new(ScriptedEngine::new()))),
    ));
    runtimes.settle();

    Arc::new(
        AppState::default()
            .with(runtimes)
            .keeping(Keeper::of(&[5u8; 32]).expect("a key"))
            .serving(Arc::new(Scripted)),
    )
}

async fn send(state: &Arc<AppState>, request: Request<Body>) -> (StatusCode, Value) {
    let response = routes::router(Arc::clone(state))
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

fn json(method: &str, path: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("a request")
}

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .body(Body::empty())
        .expect("a request")
}

fn added() -> Value {
    json!({
        "name": "cloud",
        "provider": "scripted",
        "fields": { "region": "eu" },
        "secrets": { "api_key": "vendor_key_0123456789" }
    })
}

#[tokio::test]
async fn test_a_runtime_added_over_the_api_can_take_a_box() {
    let state = server();

    let (status, body) = send(&state, json("POST", "/v1/runtimes", added())).await;

    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["place"], "remote");
    assert_eq!(body["source"], "store");
    assert_eq!(body["secrets"], json!(["api_key"]));
    assert_eq!(body["fields"]["region"], "eu");

    let (status, listed) = send(&state, get("/v1/runtimes")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["runtimes"].as_array().map(Vec::len), Some(2));
}

#[tokio::test]
async fn test_a_stored_key_never_comes_back() {
    let state = server();
    send(&state, json("POST", "/v1/runtimes", added())).await;

    let (_, body) = send(&state, get("/v1/runtimes/cloud")).await;
    let printed = body.to_string();

    assert!(
        !printed.contains("vendor_key_0123456789"),
        "no route hands a key back: {printed}"
    );

    let record = state
        .store
        .get_runtime("cloud")
        .await
        .expect("asked")
        .expect("a record");
    assert!(
        !record.secrets["api_key"].as_str().contains("vendor_key"),
        "and the store holds it sealed"
    );
}

#[tokio::test]
async fn test_a_key_can_be_replaced_and_the_rest_left_alone() {
    let state = server();
    send(&state, json("POST", "/v1/runtimes", added())).await;

    let (status, body) = send(
        &state,
        json(
            "PATCH",
            "/v1/runtimes/cloud",
            json!({ "secrets": { "api_key": "vendor_key_9876543210" } }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["fields"]["region"], "eu", "what was not changed stays");

    let record = state
        .store
        .get_runtime("cloud")
        .await
        .expect("asked")
        .expect("a record");
    assert!(record.updated_at_ms >= record.created_at_ms);
}

#[tokio::test]
async fn test_a_server_that_keeps_no_secrets_refuses_to_store_one() {
    let runtimes = Runtimes::default();
    runtimes.settle();
    let state = Arc::new(
        AppState::default()
            .with(runtimes)
            .serving(Arc::new(Scripted)),
    );

    let (status, body) = send(&state, json("POST", "/v1/runtimes", added())).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|why| why.contains("COMPUTER_SERVER_SECRET_KEY")),
        "a key in the clear is worse than a refusal: {body}"
    );
}

#[tokio::test]
async fn test_the_api_adds_vendors_and_not_host_engines() {
    let state = server();

    let (status, body) = send(
        &state,
        json(
            "POST",
            "/v1/runtimes",
            json!({ "name": "mine", "provider": "podman" }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|why| why.contains("host engine")),
        "{body}"
    );
}

#[tokio::test]
async fn test_a_name_something_else_holds_is_refused() {
    let state = server();

    let (status, _) = send(
        &state,
        json(
            "POST",
            "/v1/runtimes",
            json!({
                "name": "docker",
                "provider": "scripted",
                "secrets": { "api_key": "vendor_key_0123456789" }
            }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_an_endpoint_the_internet_cannot_reach_is_refused() {
    let state = server();

    for endpoint in [
        "http://vendor.example.com",
        "https://127.0.0.1:8080",
        "https://10.1.2.3",
        "https://vendor.internal",
    ] {
        let (status, body) = send(
            &state,
            json(
                "POST",
                "/v1/runtimes",
                json!({
                    "name": "cloud",
                    "provider": "scripted",
                    "fields": { "endpoint": endpoint },
                    "secrets": { "api_key": "vendor_key_0123456789" }
                }),
            ),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST, "{endpoint}: {body}");
    }
}

#[tokio::test]
async fn test_a_runtime_from_the_environment_is_not_changed_over_the_api() {
    let state = server();

    for (method, body) in [("PATCH", json!({ "fields": {} })), ("DELETE", Value::Null)] {
        let request = match method {
            "DELETE" => Request::builder()
                .method("DELETE")
                .uri("/v1/runtimes/docker")
                .body(Body::empty())
                .expect("a request"),
            _ => json(method, "/v1/runtimes/docker", body),
        };

        let (status, body) = send(&state, request).await;

        assert_eq!(status, StatusCode::BAD_REQUEST, "{method}: {body}");
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|why| why.contains("this host")),
            "{method}: {body}"
        );
    }
}

#[tokio::test]
async fn test_a_runtime_holding_a_box_is_not_removed() {
    let state = server();
    send(&state, json("POST", "/v1/runtimes", added())).await;

    state
        .store
        .put_box(&computer_storage::BoxRecord {
            id: "box_1".to_string(),
            runtime: "cloud".to_string(),
            spec: Default::default(),
            placement: Default::default(),
            width: 1280,
            height: 800,
            screens: 1,
            created_at_ms: 1_700_000_000_000,
            expires_at_ms: None,
        })
        .await
        .expect("recorded");

    let request = Request::builder()
        .method("DELETE")
        .uri("/v1/runtimes/cloud")
        .body(Body::empty())
        .expect("a request");
    let (status, body) = send(&state, request).await;

    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|why| why.contains("1 box")),
        "{body}"
    );
}

#[tokio::test]
async fn test_a_runtime_with_no_boxes_is_removed_and_forgotten() {
    let state = server();
    send(&state, json("POST", "/v1/runtimes", added())).await;

    let request = Request::builder()
        .method("DELETE")
        .uri("/v1/runtimes/cloud")
        .body(Body::empty())
        .expect("a request");
    let (status, _) = send(&state, request).await;

    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(state.runtimes.get("cloud").is_none());
    assert!(
        state
            .store
            .get_runtime("cloud")
            .await
            .expect("asked")
            .is_none(),
        "a restart must not bring back a runtime that was removed"
    );
}

#[tokio::test]
async fn test_a_stored_runtime_comes_back_after_a_restart() {
    let state = server();
    send(&state, json("POST", "/v1/runtimes", added())).await;

    let again = Runtimes::default();
    let taken = runtimes::from_store(
        &again,
        state.store.as_ref(),
        &Keeper::of(&[5u8; 32]).expect("a key"),
        &Scripted,
    )
    .await;

    assert_eq!(taken, 1);
    assert_eq!(
        again.get("cloud").expect("it came back").provider,
        "scripted"
    );
}

#[tokio::test]
async fn test_a_stored_runtime_the_server_key_cannot_open_is_listed_as_unavailable() {
    let state = server();
    send(&state, json("POST", "/v1/runtimes", added())).await;

    let again = Runtimes::default();
    runtimes::from_store(
        &again,
        state.store.as_ref(),
        &Keeper::of(&[6u8; 32]).expect("another key"),
        &Scripted,
    )
    .await;

    let held = again.get("cloud").expect("it is still listed");
    assert!(
        !held.ready(),
        "a runtime whose key will not open is named, not silently missing"
    );
}

#[tokio::test]
async fn test_a_vendor_box_is_built_from_the_image_that_vendor_holds() {
    let state = server();
    let digest = computer_types::Spec::default().digest();

    let (status, _) = send(
        &state,
        json(
            "POST",
            "/v1/runtimes",
            json!({
                "name": "cloud",
                "provider": "scripted",
                "fields": { "images": { digest.clone(): "tmpl-abc" } },
                "secrets": { "api_key": "vendor_key_0123456789" }
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let runtime = state.runtimes.get("cloud").expect("the vendor");
    assert_eq!(
        runtime.image_for(&digest).as_deref(),
        Some("tmpl-abc"),
        "a spec the vendor has an image for is launched from it"
    );
    assert!(
        runtime.image_for("another digest").is_none(),
        "and one it does not is refused by the vendor, which says how to build it"
    );
}

#[tokio::test]
async fn test_one_image_can_serve_every_spec() {
    let state = server();

    send(
        &state,
        json(
            "POST",
            "/v1/runtimes",
            json!({
                "name": "cloud",
                "provider": "scripted",
                "fields": { "image": "tmpl-one" },
                "secrets": { "api_key": "vendor_key_0123456789" }
            }),
        ),
    )
    .await;

    let runtime = state.runtimes.get("cloud").expect("the vendor");
    assert_eq!(runtime.image_for("whatever").as_deref(), Some("tmpl-one"));
}

#[tokio::test]
async fn test_a_box_on_a_vendor_is_gated_because_its_screen_is_on_the_internet() {
    let state = server();
    send(
        &state,
        json(
            "POST",
            "/v1/runtimes",
            json!({
                "name": "cloud",
                "provider": "scripted",
                "fields": { "image": "tmpl-one" },
                "secrets": { "api_key": "vendor_key_0123456789" }
            }),
        ),
    )
    .await;

    let runtime = state.runtimes.get("cloud").expect("the vendor");
    let built = runtime
        .drive(
            computer::Builder::default(),
            &computer_types::Spec::default(),
        )
        .config()
        .expect("a config");

    assert!(
        built.auth.is_gated(),
        "a spec that asks for no gate still gets one where the screen has a public URL"
    );
    assert_eq!(built.image, "tmpl-one");

    let engine = state.runtimes.get("docker").expect("the engine");
    let here = engine
        .drive(
            computer::Builder::default(),
            &computer_types::Spec::default(),
        )
        .config()
        .expect("a config");

    assert!(
        !here.auth.is_gated(),
        "and a box on this host, reachable on loopback only, is left as it was asked for"
    );
}
