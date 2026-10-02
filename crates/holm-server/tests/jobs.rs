use axum::body::Body;
use axum::http::{Request, StatusCode};
use holm::sandboxes::remote::RemoteApi;
use holm::testing::ScriptedRemote;
use holm::{Secret, testing::ScriptedEngine};
use holm_server::runtimes::{self, Runtimes, Vendors};
use holm_server::secrets::Keeper;
use holm_server::{AppState, jobs, routes};
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

async fn send(
    state: &Arc<AppState>,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("x-holm-confirm-delete", "true")
        .header("content-type", "application/json")
        .body(match body {
            Some(body) => Body::from(body.to_string()),
            None => Body::empty(),
        })
        .expect("a request");

    let response = routes::router(Arc::clone(state))
        .oneshot(request)
        .await
        .expect("answered");
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

async fn queueing(remote: ScriptedRemote) -> Arc<AppState> {
    let runtimes = Runtimes::default();
    runtimes.add(runtimes::engine(
        "docker",
        Arc::new(holm::EngineMachine::new(Arc::new(ScriptedEngine::new()))),
    ));
    runtimes.settle();

    let state = Arc::new(
        AppState::default()
            .with(runtimes)
            .keeping(Keeper::of(&[7u8; 32]).expect("a key"))
            .serving(Arc::new(Scripted(Arc::new(remote))))
            .queueing(),
    );

    let (status, body) = send(
        &state,
        "POST",
        "/v1/runtimes",
        Some(json!({
            "name": "cloud",
            "provider": "scripted",
            "secrets": { "api_key": "vendor_key_0123456789" }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    state
}

async fn launch(state: &Arc<AppState>) -> String {
    let (status, body) = send(
        state,
        "POST",
        "/v1/boxes",
        Some(json!({ "placement": { "runtime": "cloud" } })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "a queued launch answers before the box is up: {body}"
    );
    assert_eq!(body["state"], "starting");
    body["id"].as_str().expect("an id").to_string()
}

#[tokio::test]
async fn test_a_queued_launch_is_starting_until_a_worker_brings_it_up() {
    let state = queueing(ScriptedRemote::new().building()).await;
    let id = launch(&state).await;

    let (status, seen) = send(&state, "GET", &format!("/v1/boxes/{id}"), None).await;
    assert_eq!(
        (status, &seen["state"]),
        (StatusCode::OK, &json!("starting"))
    );

    let (_, listed) = send(&state, "GET", "/v1/boxes", None).await;
    assert_eq!(
        listed["boxes"][0]["id"],
        id.as_str(),
        "it is listed while it starts"
    );

    let (status, refused) = send(
        &state,
        "POST",
        &format!("/v1/boxes/{id}/screens/0/viewer/ticket"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "nothing drives it yet: {refused}"
    );

    assert!(jobs::run_one(&state, "w1").await, "a worker takes the job");

    let (_, seen) = send(&state, "GET", &format!("/v1/boxes/{id}"), None).await;
    assert_eq!(seen["state"], "ready");
    let (_, listed) = send(&state, "GET", "/v1/boxes", None).await;
    assert_eq!(
        listed["boxes"].as_array().map(Vec::len),
        Some(1),
        "and it is listed once"
    );
    assert!(!jobs::run_one(&state, "w1").await, "the job is gone");
}

#[tokio::test]
async fn test_a_queued_launch_that_does_not_start_says_why() {
    let state = queueing(ScriptedRemote::new().building().failing(1, "no room")).await;
    let id = launch(&state).await;

    assert!(jobs::run_one(&state, "w1").await);

    let (status, seen) = send(&state, "GET", &format!("/v1/boxes/{id}"), None).await;
    assert_eq!((status, &seen["state"]), (StatusCode::OK, &json!("failed")));
    assert!(
        seen["reason"].as_str().is_some_and(|why| !why.is_empty()),
        "{seen}"
    );

    let (status, _) = send(&state, "DELETE", &format!("/v1/boxes/{id}"), None).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "a failed box can be removed"
    );
    let (status, _) = send(&state, "GET", &format!("/v1/boxes/{id}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_a_box_removed_while_it_starts_does_not_come_up() {
    let state = queueing(ScriptedRemote::new().building()).await;
    let id = launch(&state).await;

    let (status, _) = send(&state, "DELETE", &format!("/v1/boxes/{id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    assert!(jobs::run_one(&state, "w1").await);

    let (_, listed) = send(&state, "GET", "/v1/boxes", None).await;
    assert_eq!(
        listed["boxes"].as_array().map(Vec::len),
        Some(0),
        "{listed}"
    );
}

#[tokio::test]
async fn test_a_queued_fork_is_replayed_by_the_worker_that_starts_it() {
    let state = queueing(ScriptedRemote::new().building()).await;
    let source = launch(&state).await;
    assert!(jobs::run_one(&state, "w1").await);

    let (status, body) = send(
        &state,
        "POST",
        &format!("/v1/boxes/{source}/fork"),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["box"]["state"], "starting");
    let copy = body["box"]["id"].as_str().expect("an id").to_string();

    assert!(jobs::run_one(&state, "w1").await);

    let (_, seen) = send(&state, "GET", &format!("/v1/boxes/{copy}"), None).await;
    assert_eq!(seen["state"], "ready");
    let traced = state.store.entries(&copy, None, 50).await.expect("a trace");
    assert!(
        traced.iter().any(|entry| matches!(
            &entry.event,
            holm_api::TraceEvent::ForkedFrom { source: from, .. } if from == &source
        )),
        "the copy says what it was forked from"
    );
}

#[tokio::test]
async fn test_a_queued_image_build_says_building_until_a_worker_builds_it() {
    let state = queueing(ScriptedRemote::new().building()).await;
    let digest = holm_types::Spec::default().digest();

    let (status, body) = send(
        &state,
        "POST",
        "/v1/runtimes/cloud/image",
        Some(json!({ "spec": {} })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["state"], "building");

    let status_path = format!("/v1/runtimes/cloud/images/{digest}");
    let (_, seen) = send(&state, "GET", &status_path, None).await;
    assert_eq!(seen["state"], "building");

    let (_, again) = send(
        &state,
        "POST",
        "/v1/runtimes/cloud/image",
        Some(json!({ "spec": {} })),
    )
    .await;
    assert_eq!(again["state"], "building");

    assert!(jobs::run_one(&state, "w1").await);
    assert!(
        !jobs::run_one(&state, "w1").await,
        "asking twice for one image queues one build"
    );

    let (status, seen) = send(&state, "GET", &status_path, None).await;
    assert_eq!(status, StatusCode::OK, "{seen}");
    assert!(
        seen.get("state").is_none(),
        "a built image has no state: {seen}"
    );
    assert!(
        seen["image"]
            .as_str()
            .is_some_and(|image| !image.is_empty())
    );
}

async fn called(state: &Arc<AppState>, path: &str, bearer: &str) -> (StatusCode, Value) {
    let response = routes::router(Arc::clone(state))
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(path)
                .header("authorization", format!("Bearer {bearer}"))
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("answered");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn test_a_scheduler_runs_the_queue_and_the_periodic_work_with_the_cron_secret() {
    use holm_server::schedule::{Cron, Schedule};

    const CRON: &str = "cron_secret_0123456789";
    const OPERATOR: &str = "operator_token_0123456789";

    let open = queueing(ScriptedRemote::new().building()).await;
    let id = launch(&open).await;

    let state = Arc::new(
        AppState::split(Arc::clone(&open.store), Arc::clone(&open.frames))
            .serving(Arc::new(Scripted(Arc::new(
                ScriptedRemote::new().building(),
            ))))
            .keeping(Keeper::of(&[7u8; 32]).expect("a key"))
            .queueing()
            .scheduled(Cron {
                schedule: Schedule::External,
                secret: Some(Secret::new(CRON).expect("a secret")),
                public_url: None,
            })
            .gated(Some(Secret::new(OPERATOR).expect("a token"))),
    );
    let added = runtimes::from_store(
        &state.runtimes,
        state.store.as_ref(),
        &state.secrets,
        state.vendors.as_ref(),
    )
    .await;
    assert_eq!(
        added, 1,
        "the second server reads the runtime from the store"
    );

    let (status, _) = called(&state, "/v1/jobs/run", "not the secret at all").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, body) = called(&state, "/v1/jobs/run", CRON).await;
    assert_eq!(
        (status, &body["ran"]),
        (StatusCode::OK, &json!(1)),
        "{body}"
    );

    let (status, seen) = called(&state, &format!("/v1/boxes/{id}"), OPERATOR).await;
    assert_eq!((status, &seen["state"]), (StatusCode::OK, &json!("ready")));

    let (status, _) = called(&state, &format!("/v1/boxes/{id}"), CRON).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the cron secret opens the job routes and nothing else"
    );

    for path in ["/v1/jobs/reap", "/v1/jobs/prune"] {
        let (status, body) = called(&state, path, CRON).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }
    let (status, _) = called(&state, "/v1/jobs/reap", OPERATOR).await;
    assert_eq!(status, StatusCode::OK, "the operator may run them too");
}

#[tokio::test]
async fn test_the_pruner_leaves_a_box_that_is_still_starting() {
    let state = queueing(ScriptedRemote::new().building()).await;
    let id = launch(&state).await;

    let swept = holm_server::prune::sweep(&state).await;
    assert_eq!(
        swept.boxes, 0,
        "a queued box has no trace and no live desktop yet, and it is not a finished box"
    );

    assert!(jobs::run_one(&state, "w1").await);
    let (_, seen) = send(&state, "GET", &format!("/v1/boxes/{id}"), None).await;
    assert_eq!(seen["state"], "ready");
}
