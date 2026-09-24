use axum::body::Body;
use axum::http::{Request, StatusCode};
use computer::sandboxes::remote::RemoteApi;
use computer::testing::{ScriptedEngine, ScriptedRemote};
use computer::{EngineMachine, Secret};
use computer_server::runtimes::{self, Runtimes, Vendors};
use computer_server::secrets::Keeper;
use computer_server::{AppState, routes};
use computer_storage::ImageRecord;
use computer_types::Spec;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

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

fn server(vendor: Arc<ScriptedRemote>) -> Arc<AppState> {
    let runtimes = Runtimes::default();
    runtimes.add(runtimes::engine(
        "docker",
        Arc::new(EngineMachine::new(Arc::new(ScriptedEngine::new()))),
    ));
    runtimes.settle();

    Arc::new(
        AppState::default()
            .with(runtimes)
            .keeping(Keeper::of(&[7u8; 32]).expect("a key"))
            .serving(Arc::new(Scripted(vendor))),
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

fn drop_it(path: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(path)
        .body(Body::empty())
        .expect("a request")
}

fn drop_box(id: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(format!("/v1/boxes/{id}"))
        .header("x-computer-confirm-delete", "true")
        .body(Body::empty())
        .expect("a request")
}

async fn with_cloud(vendor: Arc<ScriptedRemote>) -> Arc<AppState> {
    let state = server(vendor);
    let (status, _) = send(
        &state,
        json(
            "POST",
            "/v1/runtimes",
            json!({
                "name": "cloud",
                "provider": "scripted",
                "secrets": { "api_key": "vendor_key_0123456789" }
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    state
}

fn on_cloud() -> Value {
    json!({ "placement": { "runtime": "cloud" } })
}

#[tokio::test]
async fn test_what_a_vendor_built_is_recorded_and_used_again() {
    let vendor = Arc::new(ScriptedRemote::new().building());
    let state = with_cloud(Arc::clone(&vendor)).await;

    let (status, _) = send(&state, json("POST", "/v1/boxes", on_cloud())).await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, listed) = send(&state, get("/v1/runtimes/cloud/images")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["images"][0]["reference"], "tmpl-0");
    assert_eq!(listed["images"][0]["spec_digest"], Spec::default().digest());

    send(&state, json("POST", "/v1/boxes", on_cloud())).await;
    assert_eq!(
        vendor.built(),
        vec!["tmpl-0".to_string()],
        "the second box starts from the image the first one built"
    );
}

#[tokio::test]
async fn test_a_recorded_image_the_vendor_lost_is_built_again() {
    let vendor = Arc::new(ScriptedRemote::new().building().missing("tmpl-gone"));
    let state = with_cloud(Arc::clone(&vendor)).await;

    state
        .store
        .put_image(&ImageRecord {
            runtime: "cloud".to_string(),
            spec_digest: Spec::default().digest(),
            reference: "tmpl-gone".to_string(),
            built_at_ms: 1_700_000_000_000,
            bytes: None,
        })
        .await
        .expect("recorded");

    let (status, _) = send(&state, json("POST", "/v1/boxes", on_cloud())).await;
    assert_eq!(status, StatusCode::CREATED, "the box still starts");

    let (_, listed) = send(&state, get("/v1/runtimes/cloud/images")).await;
    assert_eq!(
        listed["images"][0]["reference"], "tmpl-0",
        "and the record names what it was built from this time"
    );
}

#[tokio::test]
async fn test_a_box_on_a_recorded_image_does_not_move_its_build_time() {
    let vendor = Arc::new(ScriptedRemote::new().building());
    let state = with_cloud(vendor).await;

    state
        .store
        .put_image(&ImageRecord {
            runtime: "cloud".to_string(),
            spec_digest: Spec::default().digest(),
            reference: "tmpl-0".to_string(),
            built_at_ms: 1_700_000_000_000,
            bytes: None,
        })
        .await
        .expect("recorded");

    send(&state, json("POST", "/v1/boxes", on_cloud())).await;

    let (_, listed) = send(&state, get("/v1/runtimes/cloud/images")).await;
    assert_eq!(
        listed["images"][0]["built_at_ms"], 1_700_000_000_000u64,
        "the record says when the image was built, not when a box last used it"
    );
}

#[tokio::test]
async fn test_an_image_a_box_runs_on_stays_until_the_box_goes() {
    let vendor = Arc::new(ScriptedRemote::new().building());
    let state = with_cloud(Arc::clone(&vendor)).await;
    let digest = Spec::default().digest();

    let (_, made) = send(&state, json("POST", "/v1/boxes", on_cloud())).await;
    let id = made["id"].as_str().expect("an id").to_string();

    let (status, _) = send(
        &state,
        drop_it(&format!("/v1/runtimes/cloud/images/{digest}")),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (status, body) = send(&state, drop_box(&id)).await;
    assert!(status.is_success(), "the box goes: {status} {body}");

    let (status, _) = send(
        &state,
        drop_it(&format!("/v1/runtimes/cloud/images/{digest}")),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        vendor.built().is_empty(),
        "the vendor holds no image nothing here records"
    );

    let (_, listed) = send(&state, get("/v1/images")).await;
    assert_eq!(listed["images"].as_array().map(Vec::len), Some(0));
}
