//! The HTTP surface.
//!
//! Everything a box can be asked is reachable here, because REST being the
//! complete surface is the promise: a shell script with `curl` and an MCP
//! server built by mapping tools onto endpoints both have to work without an
//! SDK in between.

use crate::error::{ApiError, ApiResult};
use crate::extract::ApiJson;
use crate::registry::Entry;
use crate::spec;
use crate::wire::*;
use crate::{AppState, idempotency::Replies};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use computer::{Button, Delta, Desktop as EngineDesktop, Point as EnginePoint, Selection};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const IDEMPOTENCY_KEY: &str = "idempotency-key";
/// Deleting a box is not recoverable, and the caller is usually an agent.
const CONFIRM_DELETE: &str = "x-computer-confirm-delete";

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/boxes", get(list_boxes).post(create_box))
        .route("/v1/boxes/{id}", get(get_box).delete(delete_box))
        .route("/v1/boxes/{id}/exec", post(exec))
        .route("/v1/boxes/{id}/files", get(read_file).put(write_file))
        .route("/v1/boxes/{id}/screens/{screen}/actions", post(actions))
        .route("/v1/boxes/{id}/screens/{screen}/frame", get(frame))
        .route("/v1/boxes/{id}/screens/{screen}/cursor", get(cursor))
        .route(
            "/v1/boxes/{id}/screens/{screen}/clipboard",
            get(get_clipboard).put(set_clipboard),
        )
        .route(
            "/v1/boxes/{id}/screens/{screen}/takeover",
            post(start_takeover).delete(end_takeover),
        )
        .route("/v1/boxes/{id}/screens/{screen}/viewers", get(viewers))
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true }))
}

async fn create_box(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<CreateBox>,
) -> ApiResult<Response> {
    let key = header(&headers, IDEMPOTENCY_KEY);
    if let Some(replayed) = replay(&state.replies, key.as_deref()) {
        return Ok(replayed);
    }

    let digest = body.spec.digest();
    let id = new_id();
    let (builder, resolved) = spec::plan(&body.spec, &body.placement, &id)?;

    tracing::info!(%id, %digest, "launching a box");
    let computer = builder.launch().await?;

    let entry = state
        .registry
        .insert(
            id,
            digest,
            resolved.screens,
            resolved.width,
            resolved.height,
            computer,
        )
        .await;

    answer(
        &state.replies,
        key.as_deref(),
        StatusCode::CREATED,
        &view_of(&entry),
    )
}

async fn list_boxes(State(state): State<Arc<AppState>>) -> Json<BoxList> {
    let boxes = state
        .registry
        .list()
        .await
        .iter()
        .map(|entry| view_of(entry))
        .collect();

    Json(BoxList { boxes })
}

async fn get_box(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<BoxView>> {
    let entry = state.registry.get(&id).await?;
    Ok(Json(view_of(&entry)))
}

async fn delete_box(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    if header(&headers, CONFIRM_DELETE).is_none() {
        return Err(ApiError::bad_request(format!(
            "removing a box takes the {CONFIRM_DELETE} header: its files do not come back"
        )));
    }

    state.registry.remove(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn actions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((id, screen)): Path<(String, u32)>,
    ApiJson(batch): ApiJson<ActionBatch>,
) -> ApiResult<Response> {
    let key = header(&headers, IDEMPOTENCY_KEY);
    if let Some(replayed) = replay(&state.replies, key.as_deref()) {
        return Ok(replayed);
    }

    let entry = state.registry.get(&id).await?;
    let lock = entry.screen_lock(screen).await;
    let _held = lock.lock().await;

    let target = entry.desktop(screen).await?;
    let desktop = target.as_desktop();

    let mut results = Vec::with_capacity(batch.actions.len());
    let mut stopped_at = None;

    for (index, action) in batch.actions.iter().enumerate() {
        match run(desktop, target.as_screen(), action).await {
            Ok(()) => results.push(ActionResult {
                index,
                ok: true,
                error: None,
            }),
            Err(error) => {
                // Stop here. A click that follows a move which failed lands
                // wherever the pointer was, and the frame afterwards looks
                // like it worked.
                results.push(ActionResult {
                    index,
                    ok: false,
                    error: Some(error.body),
                });
                stopped_at = Some(index);
                break;
            }
        }
    }

    if let Some(ms) = batch.settle_ms {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }

    let frame = if batch.want.contains(&Want::Frame) {
        Some(capture(&entry, screen, desktop, batch.have_frame.as_deref()).await?)
    } else {
        None
    };

    let cursor = if batch.want.contains(&Want::Cursor) {
        desktop.cursor().await.ok().map(point_out)
    } else {
        None
    };

    answer(
        &state.replies,
        key.as_deref(),
        StatusCode::OK,
        &BatchResult {
            results,
            stopped_at,
            frame,
            cursor,
        },
    )
}

async fn run(
    desktop: &dyn EngineDesktop,
    screen: Option<&computer::Screen>,
    action: &Action,
) -> ApiResult<()> {
    match action {
        Action::Move { to } => desktop.move_to(point_in(*to)).await?,
        Action::Click { at, button } => {
            let at = at.map(point_in).unwrap_or(desktop.cursor().await?);
            desktop.click(at, button_in(*button)).await?;
        }
        Action::DoubleClick { at, button } => {
            let at = at.map(point_in).unwrap_or(desktop.cursor().await?);
            desktop.double_click(at, button_in(*button)).await?;
        }
        Action::Drag { from, to, button } => {
            desktop
                .drag(point_in(*from), point_in(*to), button_in(*button))
                .await?
        }
        Action::Type { text } => desktop.type_text(text).await?,
        Action::Key { chord } => desktop.key(chord).await?,
        Action::Scroll { at, dx, dy } => {
            desktop
                .scroll(point_in(*at), Delta { dx: *dx, dy: *dy })
                .await?
        }
        Action::OpenUrl { url } => {
            let screen = screen.ok_or_else(|| {
                ApiError::bad_request("this screen has no browser to open a page in")
            })?;
            screen.open_url(url).await?;
        }
        Action::Wait { ms } => tokio::time::sleep(Duration::from_millis(*ms)).await,
    }

    Ok(())
}

#[derive(Debug, Deserialize)]
struct FrameQuery {
    #[serde(default)]
    have: Option<String>,
}

async fn frame(
    State(state): State<Arc<AppState>>,
    Path((id, screen)): Path<(String, u32)>,
    Query(query): Query<FrameQuery>,
) -> ApiResult<Json<Frame>> {
    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;

    let frame = capture(&entry, screen, target.as_desktop(), query.have.as_deref()).await?;
    Ok(Json(frame))
}

/// A desktop is mostly still between steps, so a caller already holding this
/// picture is told so rather than sent it again.
async fn capture(
    entry: &Entry,
    screen: u32,
    desktop: &dyn EngineDesktop,
    have: Option<&str>,
) -> ApiResult<Frame> {
    let png = desktop.screenshot().await?;

    let mut hasher = Sha256::new();
    hasher.update(&png);
    let hash = format!("{:x}", hasher.finalize());

    entry.remember_frame(screen, &hash).await;

    if have == Some(hash.as_str()) {
        return Ok(Frame {
            hash,
            unchanged: true,
            png_base64: None,
        });
    }

    Ok(Frame {
        hash,
        unchanged: false,
        png_base64: Some(BASE64.encode(&png)),
    })
}

async fn cursor(
    State(state): State<Arc<AppState>>,
    Path((id, screen)): Path<(String, u32)>,
) -> ApiResult<Json<Point>> {
    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;

    Ok(Json(point_out(target.as_desktop().cursor().await?)))
}

#[derive(Debug, Default, Deserialize)]
struct SelectionQuery {
    #[serde(default)]
    selection: ClipboardSelection,
}

async fn get_clipboard(
    State(state): State<Arc<AppState>>,
    Path((id, screen)): Path<(String, u32)>,
    Query(query): Query<SelectionQuery>,
) -> ApiResult<Json<ClipboardView>> {
    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;
    let screen = target
        .as_screen()
        .ok_or_else(|| ApiError::internal("this screen has no clipboard"))?;

    let text = screen.selection(selection_in(query.selection)).await?;
    Ok(Json(ClipboardView { text }))
}

async fn set_clipboard(
    State(state): State<Arc<AppState>>,
    Path((id, screen)): Path<(String, u32)>,
    ApiJson(body): ApiJson<SetClipboard>,
) -> ApiResult<StatusCode> {
    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;
    let screen = target
        .as_screen()
        .ok_or_else(|| ApiError::internal("this screen has no clipboard"))?;

    screen
        .set_selection(selection_in(body.selection), &body.text)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn start_takeover(
    State(state): State<Arc<AppState>>,
    Path((id, screen)): Path<(String, u32)>,
    ApiJson(body): ApiJson<TakeoverRequest>,
) -> ApiResult<Json<TakeoverView>> {
    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;
    let held = target
        .as_screen()
        .ok_or_else(|| ApiError::internal("this screen cannot be handed over"))?;

    let takeover = if body.shared {
        held.share().await?
    } else {
        held.hand_over().await?
    };

    Ok(Json(TakeoverView {
        url: takeover.url().map(str::to_string),
        exclusive: takeover.exclusive(),
        screen,
    }))
}

/// Through `reclaim` rather than `Takeover::end`: the handle that started it
/// belonged to a request that has already returned. The token that says who is
/// driving lives in the box, which is what makes this possible.
async fn end_takeover(
    State(state): State<Arc<AppState>>,
    Path((id, screen)): Path<(String, u32)>,
) -> ApiResult<StatusCode> {
    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;
    let held = target
        .as_screen()
        .ok_or_else(|| ApiError::internal("this screen cannot be reclaimed"))?;

    held.reclaim().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn viewers(
    State(state): State<Arc<AppState>>,
    Path((id, screen)): Path<(String, u32)>,
) -> ApiResult<Json<ViewersView>> {
    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;
    let held = target
        .as_screen()
        .ok_or_else(|| ApiError::internal("this screen has no viewer"))?;

    let counts = held.viewers().await?;
    Ok(Json(ViewersView {
        watching: counts.watching,
        driving: counts.driving,
        person_driving: counts.person_present(),
    }))
}

async fn exec(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<ExecRequest>,
) -> ApiResult<Json<ExecResponse>> {
    if body.argv.is_empty() {
        return Err(ApiError::bad_request("argv is empty"));
    }

    let entry = state.registry.get(&id).await?;
    let result = match body.timeout_ms {
        Some(ms) => {
            entry
                .computer
                .exec_within(&body.argv, Duration::from_millis(ms))
                .await?
        }
        None => entry.computer.exec(&body.argv).await?,
    };

    Ok(Json(ExecResponse {
        code: result.code,
        stdout: result.stdout_utf8(),
        stderr: result.stderr_utf8(),
        timed_out: result.timed_out,
    }))
}

#[derive(Debug, Deserialize)]
struct PathQuery {
    path: String,
}

async fn read_file(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<PathQuery>,
) -> ApiResult<Json<ReadFile>> {
    let entry = state.registry.get(&id).await?;
    let bytes = entry.computer.read_file(&query.path).await?;

    Ok(Json(ReadFile {
        path: query.path,
        contents_base64: BASE64.encode(bytes),
    }))
}

async fn write_file(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<WriteFile>,
) -> ApiResult<StatusCode> {
    let bytes = BASE64
        .decode(body.contents_base64.as_bytes())
        .map_err(|error| {
            ApiError::bad_request(format!("contents_base64 is not base64: {error}"))
        })?;

    let entry = state.registry.get(&id).await?;
    entry.computer.write_file(&body.path, &bytes).await?;
    Ok(StatusCode::NO_CONTENT)
}

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

fn replay(replies: &Replies, key: Option<&str>) -> Option<Response> {
    let (status, body) = replies.get(key?)?;
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);

    Some(
        (
            status,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            body,
        )
            .into_response(),
    )
}

/// Keeps the answer where a retry of the same key will find it.
fn answer<T: serde::Serialize>(
    replies: &Replies,
    key: Option<&str>,
    status: StatusCode,
    value: &T,
) -> ApiResult<Response> {
    let body = serde_json::to_vec(value)
        .map_err(|error| ApiError::internal(format!("the answer would not serialise: {error}")))?;

    if let Some(key) = key {
        replies.put(key, status.as_u16(), body.clone());
    }

    Ok((
        status,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response())
}

fn view_of(entry: &Entry) -> BoxView {
    BoxView {
        id: entry.id.clone(),
        spec_digest: entry.spec_digest.clone(),
        state: BoxState::Ready,
        screens: entry.screens,
        width: entry.width,
        height: entry.height,
        viewer_url: entry.computer.viewer_url(),
        devtools_url: entry
            .computer
            .devtools()
            .map(|endpoint| endpoint.http_url.clone()),
        created_at_ms: millis(entry.created_at),
        expires_at_ms: entry.computer.expires_at().map(millis),
    }
}

fn millis(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default()
}

fn new_id() -> String {
    let mut bytes = [0u8; 16];
    // A box id names a thing anyone who can reach this API can drive, so it
    // comes from the CSPRNG rather than from the clock.
    if getrandom::fill(&mut bytes).is_err() {
        return format!("box_{}", millis(SystemTime::now()));
    }

    let mut id = String::from("box_");
    for byte in bytes {
        id.push_str(&format!("{byte:02x}"));
    }
    id
}

fn point_in(point: Point) -> EnginePoint {
    EnginePoint::new(point.x, point.y)
}

fn point_out(point: EnginePoint) -> Point {
    Point {
        x: point.x,
        y: point.y,
    }
}

fn button_in(button: MouseButton) -> Button {
    match button {
        MouseButton::Left => Button::Left,
        MouseButton::Right => Button::Right,
        MouseButton::Middle => Button::Middle,
    }
}

fn selection_in(selection: ClipboardSelection) -> Selection {
    match selection {
        ClipboardSelection::Clipboard => Selection::Clipboard,
        ClipboardSelection::Primary => Selection::Primary,
    }
}
