//! The HTTP surface.
//!
//! Everything a box can be asked is reachable here, because REST being the
//! complete surface is the promise: a shell script with `curl` and an MCP
//! server built by mapping tools onto endpoints both have to work without an
//! SDK in between.

use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiPath, ApiQuery};
use crate::idempotency::{self, Lookup, Replies};
use crate::registry::{AsDesktop, Entry};
use crate::spec;
use crate::trace::Trace;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use computer::{
    Button as EngineButton, Delta, Desktop as EngineDesktop, Point as EnginePoint,
    Selection as EngineSelection,
};
use computer_api::*;
use computer_types::Spec;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::collections::btree_map::Entry as Entry_;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const HEALTH: &str = "/v1/health";
const IDEMPOTENCY_KEY: &str = "idempotency-key";
/// Deleting a box is not recoverable, and the caller is usually an agent.
const CONFIRM_DELETE: &str = "x-computer-confirm-delete";
const TRACE_PAGE: usize = 500;
/// The longest a replay is given before it stops and says so. A fork is one
/// HTTP request, and a box that was driven for an hour cannot take one.
const REPLAY_BUDGET: Duration = Duration::from_secs(180);
/// The most page text one read answers with.
///
/// A ceiling rather than a default a caller can raise: `limit` is there to ask
/// for less than this, and a page is unbounded.
const PAGE_TEXT: usize = 20_000;
const LINKS: usize = 100;
/// The most matches one find answers with.
const FOUND: usize = 50;
/// How many tabs a box keeps. Enough that a caller can come back to what it
/// opened a few steps ago, few enough that a long run does not bury the
/// browser.
const TABS: usize = 12;
/// How long a wait runs by default, and the longest one it can be asked for:
/// a request holds a connection while it waits.
const WAIT_MS: u64 = 10_000;
const MAX_WAIT: Duration = Duration::from_secs(60);
/// The most of an original pause a replay reproduces. Pacing matters — a page
/// that had two seconds to load gets them — but an idle hour does not.
const REPLAY_GAP_CAP: Duration = Duration::from_secs(2);
/// A ceiling on any pause a request can ask for. The screen lock is held
/// across a settle and across a wait, so an uncapped one from a single caller
/// is a screen no other request can reach again.
const MAX_PAUSE: Duration = Duration::from_secs(30);

/// How long a screen has to hold still before a `wait_still` calls it settled,
/// and how long it may go on waiting. Both are clamped by [`MAX_PAUSE`], so a
/// caller cannot hold the screen lock for the length of a lease.
const SETTLE: u64 = 400;
const STILL: u64 = 10_000;
/// A ceiling on a command's own limit, above the engine's two-minute default
/// but short of holding a connection open indefinitely.
const MAX_EXEC: Duration = Duration::from_secs(600);

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(HEALTH, get(health))
        .route("/v1/boxes", get(list_boxes).post(create_box))
        .route("/v1/boxes/{id}", get(get_box).delete(delete_box))
        .route("/v1/boxes/{id}/fork", post(fork))
        .route("/v1/boxes/{id}/exec", post(exec))
        .route("/v1/boxes/{id}/trace", get(read_trace))
        .route("/v1/boxes/{id}/trace/frames/{hash}", get(trace_frame))
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
        .route("/v1/boxes/{id}/screens/{screen}/windows", get(list_windows))
        .route(
            "/v1/boxes/{id}/screens/{screen}/windows/active",
            get(active_window),
        )
        .route(
            "/v1/boxes/{id}/screens/{screen}/windows/wait",
            post(await_window),
        )
        .route(
            "/v1/boxes/{id}/screens/{screen}/windows/{window}/focus",
            post(focus_window),
        )
        .route(
            "/v1/boxes/{id}/screens/{screen}/windows/{window}/arrange",
            post(arrange_window),
        )
        .route(
            "/v1/boxes/{id}/screens/{screen}/windows/{window}",
            axum::routing::delete(close_window),
        )
        .route("/v1/catalog", get(catalog))
        .route("/v1/boxes/{id}/page", get(read_page))
        .route("/v1/boxes/{id}/page/find", get(find_elements))
        .route("/v1/boxes/{id}/page/element", post(on_element))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&state),
            crate::auth::gate,
        ))
        .with_state(state)
}

async fn health() -> Json<Health> {
    Json(Health {
        ok: true,
        service: computer_api::SERVICE.to_string(),
    })
}

async fn create_box(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<CreateBox>,
) -> ApiResult<Response> {
    let stamp = Idempotent::of(&headers, "POST /v1/boxes", &body);
    if let Some(replayed) = stamp.replay(&state.replies)? {
        return Ok(replayed);
    }

    let digest = body.spec.digest();
    let id = new_id();
    let (builder, resolved) = spec::plan(&body.spec, &body.placement, &id)?;
    let builder = through(builder, &state);

    tracing::info!(%id, %digest, "launching a box");
    let computer = builder.launch().await?;

    let entry = state
        .registry
        .insert(
            id,
            body.spec.clone(),
            resolved.screens,
            resolved.width,
            resolved.height,
            computer,
        )
        .await;

    state.traces.of(&entry.id).record(
        Actor::Agent,
        TraceEvent::BoxCreated {
            spec_digest: entry.spec_digest(),
            spec: Box::new(body.spec.clone()),
            placement: Box::new(body.placement.clone()),
            width: resolved.width,
            height: resolved.height,
            screens: resolved.screens,
        },
    );

    stamp.answer(&state.replies, StatusCode::CREATED, &view_of(&entry))
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
    ApiPath(id): ApiPath<String>,
) -> ApiResult<Json<BoxView>> {
    let entry = state.registry.get(&id).await?;
    Ok(Json(view_of(&entry)))
}

async fn delete_box(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<StatusCode> {
    if header(&headers, CONFIRM_DELETE).is_none() {
        return Err(ApiError::bad_request(format!(
            "removing a box takes the {CONFIRM_DELETE} header: its files do not come back"
        )));
    }

    state.registry.remove(&id).await?;
    state
        .traces
        .of(&id)
        .record(Actor::Agent, TraceEvent::BoxDeleted);

    Ok(StatusCode::NO_CONTENT)
}

async fn actions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ApiPath((id, screen)): ApiPath<(String, u32)>,
    ApiJson(batch): ApiJson<ActionBatch>,
) -> ApiResult<Response> {
    let stamp = Idempotent::of(
        &headers,
        &format!("POST /v1/boxes/{id}/screens/{screen}/actions"),
        &batch,
    );
    if let Some(replayed) = stamp.replay(&state.replies)? {
        return Ok(replayed);
    }

    let entry = state.registry.get(&id).await?;
    let trace = state.traces.of(&id);
    let lock = entry.screen_lock(screen).await?;
    let _held = lock.lock().await;

    let target = entry.desktop(screen).await?;
    let desktop = target.as_desktop();

    // Resolved when the first page action asks, not before: `open_url` raises a
    // new tab, so a handle taken up front would address the one it replaced.
    let browser = entry.computer.browser();
    let mut page = None;

    let mut results = Vec::with_capacity(batch.actions.len());
    let mut windows = Vec::new();
    let mut stopped_at = None;

    for (index, action) in batch.actions.iter().enumerate() {
        let outcome = run(
            desktop,
            target.as_screen(),
            action,
            &entry.spec,
            browser.as_ref(),
            &mut page,
        )
        .await;

        // Its own event: a name and its arguments are the whole launch.
        match (&outcome, action) {
            (Ok(Some(window)), Action::Launch { app, args }) => trace.record(
                Actor::Agent,
                TraceEvent::AppLaunched {
                    screen,
                    app: app.clone(),
                    args: args.clone(),
                    window: window.id.clone(),
                },
            ),
            _ => trace.record(
                Actor::Agent,
                TraceEvent::Acted {
                    screen,
                    action: action.clone(),
                    ok: outcome.is_ok(),
                    error: outcome.as_ref().err().map(|error| error.body.clone()),
                },
            ),
        };

        match outcome {
            Ok(window) => {
                windows.extend(window);
                results.push(ActionResult {
                    index,
                    ok: true,
                    error: None,
                })
            }
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
        tokio::time::sleep(Duration::from_millis(ms).min(MAX_PAUSE)).await;
    }

    let frame = if batch.want.contains(&Want::Frame) {
        Some(
            capture(
                &trace,
                Actor::Agent,
                screen,
                desktop,
                batch.have_frame.as_deref(),
            )
            .await?,
        )
    } else {
        None
    };

    let cursor = if batch.want.contains(&Want::Cursor) {
        desktop.cursor().await.ok().map(point_out)
    } else {
        None
    };

    stamp.answer(
        &state.replies,
        StatusCode::OK,
        &BatchResult {
            results,
            windows,
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
    spec: &Spec,
    browser: Option<&computer::Devtools>,
    page: &mut Option<computer::Page>,
) -> ApiResult<Option<Window>> {
    match action {
        Action::Move { to } => desktop.move_to(point_in(*to)).await?,
        Action::Click { at, button, held } => {
            let at = at.map(point_in).unwrap_or(desktop.cursor().await?);
            desktop
                .click_with(at, button_in(*button), &held_in(held))
                .await?;
        }
        Action::DoubleClick { at, button } => {
            let at = at.map(point_in).unwrap_or(desktop.cursor().await?);
            desktop.double_click(at, button_in(*button)).await?;
        }
        Action::Drag {
            from,
            to,
            button,
            held,
        } => {
            desktop
                .drag_with(
                    point_in(*from),
                    point_in(*to),
                    button_in(*button),
                    &held_in(held),
                )
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

            // A new tab, raised in front of the last one. Whatever page action
            // follows wants that one, not the page this batch started on.
            *page = None;

            // And the ones before it do not accumulate. Best effort: a browser
            // that would not say what it holds is not a reason to refuse the
            // page that just opened.
            if let Some(browser) = browser {
                let _ = browser.tidy(TABS).await;
            }
        }
        Action::Wait { ms } => tokio::time::sleep(Duration::from_millis(*ms).min(MAX_PAUSE)).await,
        Action::WaitStill {
            settle_ms,
            within_ms,
        } => {
            let settle = Duration::from_millis(settle_ms.unwrap_or(SETTLE)).min(MAX_PAUSE);
            let within = Duration::from_millis(within_ms.unwrap_or(STILL)).min(MAX_PAUSE);

            desktop.wait_until_still(settle, within).await?
        }
        Action::OnPage { what } => {
            if page.is_none() {
                let browser = browser
                    .ok_or_else(|| ApiError::bad_request("this box publishes no DevTools port"))?;

                *page = browser.visible_page().await?;
            }

            let page = page
                .as_mut()
                .ok_or_else(|| ApiError::not_found("no page is on screen"))?;

            apply(page, what.clone()).await?;
        }
        Action::Launch { app, args } => {
            let screen =
                screen.ok_or_else(|| ApiError::bad_request("this screen cannot start an app"))?;
            let known = computer::apps::resolve(spec, app)?;

            let Some(computer_types::WindowMatch::Class(class)) = known.window else {
                return Err(ApiError::bad_request(format!(
                    "{app} names no window class, so a launch could not tell \
                     when it had drawn"
                )));
            };

            let mut command = known.command.clone();
            command.extend(args.iter().cloned());

            let window = screen
                .launch(&computer::Launch {
                    command,
                    class,
                    settle: Duration::from_millis(
                        known.settle_ms.unwrap_or(computer::apps::SETTLE_MS),
                    ),
                    within: Duration::from_millis(computer::apps::READY_MS),
                })
                .await?;

            return Ok(Some(window_out(window)));
        }
    }

    Ok(None)
}

/// Flat rather than nested, because this is a GET and a query string has no
/// shape: `?window=42&scale=50`.
#[derive(Debug, Deserialize)]
struct FrameQuery {
    #[serde(default)]
    have: Option<String>,
    #[serde(default)]
    window: Option<String>,
    #[serde(default)]
    x: Option<u32>,
    #[serde(default)]
    y: Option<u32>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    scale: Option<u32>,
}

impl FrameQuery {
    fn shot(&self) -> ApiResult<Shot> {
        let region = match (self.x, self.y, self.width, self.height) {
            (None, None, None, None) => None,
            (Some(x), Some(y), Some(width), Some(height)) => Some(Region {
                at: Point { x, y },
                width,
                height,
            }),
            // Half a rectangle would otherwise be read as a corner and a
            // guess, and answered with a picture of the wrong thing.
            _ => {
                return Err(ApiError::bad_request(
                    "a region takes x, y, width and height together",
                ));
            }
        };

        if region.is_some() && self.window.is_some() {
            return Err(ApiError::bad_request(
                "a capture is of a window or of a region, not both",
            ));
        }

        Ok(Shot {
            window: self.window.clone(),
            region,
            scale: self.scale,
        })
    }
}

async fn frame(
    State(state): State<Arc<AppState>>,
    ApiPath((id, screen)): ApiPath<(String, u32)>,
    ApiQuery(query): ApiQuery<FrameQuery>,
) -> ApiResult<Json<Frame>> {
    // Before the box is looked up: a query that does not make sense is a bad
    // request whether or not the box behind it is there.
    let shot = query.shot()?;

    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;

    let png = match shot.is_whole() {
        true => target.as_desktop().screenshot().await?,
        // Everything narrower goes through the screen, which is where a
        // window becomes a rectangle and where the sizes are checked.
        false => {
            let held = target
                .as_screen()
                .ok_or_else(|| ApiError::bad_request("this screen cannot be captured in part"))?;

            held.capture(&shot_in(&shot)).await?
        }
    };

    let trace = state.traces.of(&id);

    Ok(Json(recorded(
        &trace,
        Actor::Agent,
        screen,
        png,
        query.have.as_deref(),
    )))
}

fn held_in(held: &[Held]) -> Vec<computer::Held> {
    held.iter()
        .map(|one| match one {
            Held::Shift => computer::Held::Shift,
            Held::Ctrl => computer::Held::Ctrl,
            Held::Alt => computer::Held::Alt,
            Held::Super => computer::Held::Super,
        })
        .collect()
}

fn shot_in(shot: &Shot) -> computer::Shot {
    let of = match (&shot.window, &shot.region) {
        (Some(window), _) => computer::Of::Window(window.clone()),
        (None, Some(area)) => computer::Of::Region(computer::Rect::new(
            point_in(area.at),
            area.width,
            area.height,
        )),
        _ => computer::Of::Screen,
    };

    computer::Shot {
        of,
        scale: shot.scale,
    }
}

/// A desktop is mostly still between steps, so a caller already holding this
/// picture is told so rather than sent it again.
async fn capture(
    trace: &Trace,
    actor: Actor,
    screen: u32,
    desktop: &dyn EngineDesktop,
    have: Option<&str>,
) -> ApiResult<Frame> {
    let png = desktop.screenshot().await?;

    Ok(recorded(trace, actor, screen, png, have))
}

/// Left out altogether when the caller already holds it.
fn recorded(trace: &Trace, actor: Actor, screen: u32, png: Vec<u8>, have: Option<&str>) -> Frame {
    let mut hasher = Sha256::new();
    hasher.update(&png);
    let hash = format!("{:x}", hasher.finalize());

    trace.note_frame(actor, screen, &hash, &png);

    if have == Some(hash.as_str()) {
        return Frame {
            hash,
            unchanged: true,
            png_base64: None,
        };
    }

    Frame {
        hash,
        unchanged: false,
        png_base64: Some(BASE64.encode(&png)),
    }
}

async fn cursor(
    State(state): State<Arc<AppState>>,
    ApiPath((id, screen)): ApiPath<(String, u32)>,
) -> ApiResult<Json<Point>> {
    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;

    Ok(Json(point_out(target.as_desktop().cursor().await?)))
}

#[derive(Debug, Default, Deserialize)]
struct SelectionQuery {
    #[serde(default)]
    selection: Selection,
}

async fn get_clipboard(
    State(state): State<Arc<AppState>>,
    ApiPath((id, screen)): ApiPath<(String, u32)>,
    ApiQuery(query): ApiQuery<SelectionQuery>,
) -> ApiResult<Json<ClipboardView>> {
    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;
    let held = target
        .as_screen()
        .ok_or_else(|| ApiError::internal("this screen has no clipboard"))?;

    let text = held.selection(selection_in(query.selection)).await?;

    state.traces.of(&id).record(
        Actor::Agent,
        TraceEvent::ClipboardRead {
            screen,
            selection: query.selection,
        },
    );

    Ok(Json(ClipboardView { text }))
}

async fn set_clipboard(
    State(state): State<Arc<AppState>>,
    ApiPath((id, screen)): ApiPath<(String, u32)>,
    ApiJson(body): ApiJson<SetClipboard>,
) -> ApiResult<StatusCode> {
    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;
    let held = target
        .as_screen()
        .ok_or_else(|| ApiError::internal("this screen has no clipboard"))?;

    held.set_selection(selection_in(body.selection), &body.text)
        .await?;

    state.traces.of(&id).record(
        Actor::Agent,
        TraceEvent::ClipboardSet {
            screen,
            selection: body.selection,
        },
    );

    Ok(StatusCode::NO_CONTENT)
}

async fn start_takeover(
    State(state): State<Arc<AppState>>,
    ApiPath((id, screen)): ApiPath<(String, u32)>,
    ApiJson(body): ApiJson<TakeoverRequest>,
) -> ApiResult<Json<TakeoverView>> {
    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;
    let held = target
        .as_screen()
        .ok_or_else(|| ApiError::internal("this screen cannot be handed over"))?;

    let trace = state.traces.of(&id);
    // The frame the person is being given, so what they changed is the
    // difference between this and the one taken when they hand it back.
    let _ = capture(&trace, Actor::Agent, screen, target.as_desktop(), None).await;

    let takeover = if body.shared {
        held.share().await?
    } else {
        held.hand_over().await?
    };

    trace.record(
        Actor::Agent,
        TraceEvent::TakeoverStarted {
            screen,
            exclusive: takeover.exclusive(),
        },
    );

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
    ApiPath((id, screen)): ApiPath<(String, u32)>,
) -> ApiResult<StatusCode> {
    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;
    let held = target
        .as_screen()
        .ok_or_else(|| ApiError::internal("this screen cannot be reclaimed"))?;

    let trace = state.traces.of(&id);
    // Taken while the screen is still theirs — reading is allowed during a
    // handover — so the frame is what the person left rather than what
    // happened after they let go.
    let _ = capture(&trace, Actor::Person, screen, target.as_desktop(), None).await;

    held.reclaim().await?;
    trace.record(Actor::Agent, TraceEvent::TakeoverEnded { screen });

    Ok(StatusCode::NO_CONTENT)
}

async fn viewers(
    State(state): State<Arc<AppState>>,
    ApiPath((id, screen)): ApiPath<(String, u32)>,
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
    ApiPath(id): ApiPath<String>,
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
                .exec_within(&body.argv, Duration::from_millis(ms).min(MAX_EXEC))
                .await?
        }
        None => entry.computer.exec(&body.argv).await?,
    };

    state.traces.of(&id).record(
        Actor::Agent,
        TraceEvent::Executed {
            argv: body.argv.clone(),
            code: result.code,
            timed_out: result.timed_out,
        },
    );

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
    ApiPath(id): ApiPath<String>,
    ApiQuery(query): ApiQuery<PathQuery>,
) -> ApiResult<Json<ReadFile>> {
    let entry = state.registry.get(&id).await?;
    let bytes = entry.computer.read_file(&query.path).await?;

    state.traces.of(&id).record(
        Actor::Agent,
        TraceEvent::FileRead {
            path: query.path.clone(),
            bytes: bytes.len(),
        },
    );

    Ok(Json(ReadFile {
        path: query.path,
        contents_base64: BASE64.encode(bytes),
    }))
}

async fn write_file(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<String>,
    ApiJson(body): ApiJson<WriteFile>,
) -> ApiResult<StatusCode> {
    let bytes = BASE64
        .decode(body.contents_base64.as_bytes())
        .map_err(|error| {
            ApiError::bad_request(format!("contents_base64 is not base64: {error}"))
        })?;

    let entry = state.registry.get(&id).await?;
    entry.computer.write_file(&body.path, &bytes).await?;

    state.traces.of(&id).record(
        Actor::Agent,
        TraceEvent::FileWritten {
            path: body.path.clone(),
            bytes: bytes.len(),
        },
    );

    Ok(StatusCode::NO_CONTENT)
}

/// Build a box again from what was done to the first one.
///
/// Reads the source's trace rather than the source, so a box that has been
/// removed can still be forked: its record outlived it and carries the spec.
async fn fork(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ApiPath(id): ApiPath<String>,
    ApiJson(body): ApiJson<ForkRequest>,
) -> ApiResult<Response> {
    let stamp = Idempotent::of(&headers, &format!("POST /v1/boxes/{id}/fork"), &body);
    if let Some(replayed) = stamp.replay(&state.replies)? {
        return Ok(replayed);
    }

    if body.mode == ForkMode::Snapshot {
        return Err(ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            ErrorCode::Unsupported,
            "no substrate here can freeze a running desktop: a container \
             runtime cannot checkpoint an X session, so the copy would come \
             back to a screen that never resumed. Use replay.",
        ));
    }

    let source = state
        .traces
        .get(&id)
        .ok_or_else(|| ApiError::not_found(format!("nothing was ever traced for {id}")))?;

    let history = source.entries(None, usize::MAX);
    let (spec, placement) = history
        .iter()
        .find_map(|entry| match &entry.event {
            TraceEvent::BoxCreated {
                spec, placement, ..
            } => Some((spec.clone(), placement.clone())),
            _ => None,
        })
        .ok_or_else(|| {
            ApiError::bad_request(format!(
                "the trace for {id} does not say what box it was, so there is \
                 nothing to build again"
            ))
        })?;

    let placement = body.placement.clone().map(Box::new).unwrap_or(placement);
    let new_id = new_id();
    let (builder, resolved) = spec::plan(&spec, &placement, &new_id)?;
    let builder = through(builder, &state);

    tracing::info!(from = %id, to = %new_id, "forking a box");
    let computer = builder.launch().await?;

    let entry = state
        .registry
        .insert(
            new_id.clone(),
            (*spec).clone(),
            resolved.screens,
            resolved.width,
            resolved.height,
            computer,
        )
        .await;

    let trace = state.traces.of(&new_id);
    trace.record(
        Actor::Agent,
        TraceEvent::BoxCreated {
            spec_digest: entry.spec_digest(),
            spec,
            placement,
            width: resolved.width,
            height: resolved.height,
            screens: resolved.screens,
        },
    );
    trace.record(
        Actor::Agent,
        TraceEvent::ForkedFrom {
            source: id.clone(),
            up_to: body.up_to,
        },
    );

    let report = replay_onto(&entry, &trace, &history, body.up_to).await;

    // Close with what the replay produced, so a caller can compare it against
    // the source's own last frame. They will rarely be the same bytes: a
    // desktop animates, which is why the report counts actions rather than
    // claiming the two boxes match.
    if let Ok(target) = entry.desktop(0).await {
        let _ = capture(&trace, Actor::Agent, 0, target.as_desktop(), None).await;
    }

    stamp.answer(
        &state.replies,
        StatusCode::CREATED,
        &ForkResult {
            created: view_of(&entry),
            replay: report,
        },
    )
}

/// Do again, in order and at roughly the original pace, what the source was
/// asked to do.
async fn replay_onto(
    entry: &Entry,
    trace: &Trace,
    history: &[TraceEntry],
    up_to: Option<u64>,
) -> ReplayReport {
    let deadline = Instant::now() + REPLAY_BUDGET;
    let mut report = ReplayReport {
        attempted: 0,
        ok: 0,
        stopped_at: None,
        truncated: false,
        skipped: Vec::new(),
    };
    let mut previous: Option<u64> = None;
    // Held across the replay: `desktop` runs the image's screen start command
    // every time it is called, and a five hundred action history would spend
    // most of its budget on those rather than on the actions.
    let mut targets: BTreeMap<u32, Box<dyn AsDesktop + Send + '_>> = BTreeMap::new();

    for source in history {
        if up_to.is_some_and(|last| source.seq > last) {
            break;
        }

        let step = match &source.event {
            TraceEvent::Acted {
                screen,
                action,
                ok: true,
                ..
            } => Step::Act {
                screen: *screen,
                action: action.clone(),
            },
            // An action the original was refused is not part of what happened
            // to it, so replaying it would invent a difference.
            TraceEvent::Acted { .. } => continue,
            TraceEvent::Executed { argv, .. } if !argv.is_empty() => {
                Step::Exec { argv: argv.clone() }
            }
            // Replayable, unlike a file write: the trace holds all of it.
            TraceEvent::AppLaunched {
                screen, app, args, ..
            } => Step::Act {
                screen: *screen,
                action: Action::Launch {
                    app: app.clone(),
                    args: args.clone(),
                },
            },
            // The trace keeps what a write or a copy was about, not the bytes
            // it carried, so these cannot be done again from the record. Said
            // out loud, because a fork short of the original in a way nothing
            // reports is worse than one that says where it is short.
            TraceEvent::FileWritten { path, .. } => {
                report.skipped.push(Skipped {
                    seq: source.seq,
                    kind: "file_written".to_string(),
                    why: format!(
                        "the trace records that {path} was written, not what went into it"
                    ),
                });
                continue;
            }
            TraceEvent::ClipboardSet { selection, .. } => {
                report.skipped.push(Skipped {
                    seq: source.seq,
                    kind: "clipboard_set".to_string(),
                    why: format!("the trace records that {selection:?} was set, not the text"),
                });
                continue;
            }
            _ => continue,
        };

        if Instant::now() >= deadline {
            report.truncated = true;
            break;
        }

        if let Some(before) = previous {
            let gap = Duration::from_millis(source.at_ms.saturating_sub(before));
            tokio::time::sleep(gap.min(REPLAY_GAP_CAP)).await;
        }
        previous = Some(source.at_ms);

        report.attempted += 1;

        let outcome = match &step {
            Step::Act { screen, action } => {
                let target = match targets.entry(*screen) {
                    Entry_::Occupied(held) => Ok(held.into_mut()),
                    Entry_::Vacant(slot) => entry.desktop(*screen).await.map(|it| slot.insert(it)),
                };

                let acted = match target {
                    Ok(target) => {
                        run(
                            target.as_desktop(),
                            target.as_screen(),
                            action,
                            &entry.spec,
                            None,
                            &mut None,
                        )
                        .await
                    }
                    Err(error) => Err(error),
                };

                match (&acted, action) {
                    (Ok(Some(window)), Action::Launch { app, args }) => trace.record(
                        Actor::Agent,
                        TraceEvent::AppLaunched {
                            screen: *screen,
                            app: app.clone(),
                            args: args.clone(),
                            window: window.id.clone(),
                        },
                    ),
                    _ => trace.record(
                        Actor::Agent,
                        TraceEvent::Acted {
                            screen: *screen,
                            action: action.clone(),
                            ok: acted.is_ok(),
                            error: acted.as_ref().err().map(|error| error.body.clone()),
                        },
                    ),
                };
                acted.map(|_| ())
            }
            Step::Exec { argv } => {
                let ran = entry.computer.exec(argv).await.map_err(ApiError::from);

                if let Ok(result) = &ran {
                    trace.record(
                        Actor::Agent,
                        TraceEvent::Executed {
                            argv: argv.clone(),
                            code: result.code,
                            timed_out: result.timed_out,
                        },
                    );
                }
                ran.map(|_| ())
            }
        };

        match outcome {
            Ok(()) => report.ok += 1,
            Err(_) => {
                report.stopped_at = Some(source.seq);
                break;
            }
        }
    }

    report
}

/// One thing a replay does again.
enum Step {
    Act { screen: u32, action: Action },
    Exec { argv: Vec<String> },
}

#[derive(Debug, Deserialize)]
struct TraceQuery {
    #[serde(default)]
    after: Option<u64>,
    #[serde(default)]
    limit: Option<usize>,
}

/// Oldest first, and answers for a box that has been removed: the record is the
/// point.
async fn read_trace(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(query): ApiQuery<TraceQuery>,
) -> ApiResult<Json<TraceView>> {
    let trace = state
        .traces
        .get(&id)
        .ok_or_else(|| ApiError::not_found(format!("nothing was ever traced for {id}")))?;

    let limit = query.limit.unwrap_or(TRACE_PAGE).clamp(1, TRACE_PAGE);
    let entries = trace.entries(query.after, limit);
    let next = (entries.len() == limit).then(|| entries.last().map(|entry| entry.seq));

    Ok(Json(TraceView {
        entries,
        next: next.flatten(),
    }))
}

async fn trace_frame(
    State(state): State<Arc<AppState>>,
    ApiPath((id, hash)): ApiPath<(String, String)>,
) -> ApiResult<Response> {
    let png = state
        .traces
        .get(&id)
        .and_then(|trace| trace.frame(&hash))
        .ok_or_else(|| {
            ApiError::not_found(format!(
                "frame {hash} is not held for {id}; a trace keeps the most recent \
                 frames and older entries name one that has gone"
            ))
        })?;

    Ok((
        [(axum::http::header::CONTENT_TYPE, "image/png")],
        Body::from(png.as_slice().to_vec()),
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
struct PageQuery {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    max_links: Option<usize>,
    #[serde(default)]
    format: Option<Reading>,
}

/// What the page in front is showing, as text.
///
/// The page the screen shows, not the first one open: a caller reading what it
/// can see is the point, and a frame and this have to agree.
async fn read_page(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(query): ApiQuery<PageQuery>,
) -> ApiResult<Json<PageText>> {
    let entry = state.registry.get(&id).await?;
    let browser = entry
        .computer
        .browser()
        .ok_or_else(|| ApiError::bad_request("this box publishes no DevTools port"))?;

    let mut page = browser
        .visible_page()
        .await?
        .ok_or_else(|| ApiError::not_found("no page is on screen"))?;

    let format = match query.format.unwrap_or_default() {
        Reading::Markdown => computer::Reading::Markdown,
        Reading::Text => computer::Reading::Text,
        Reading::Raw => computer::Reading::Raw,
    };

    // Clamped here rather than in the engine: a ceiling is what a deployment
    // owes whoever it answers, and a library owes its caller none.
    let read = page
        .read(
            format,
            Some(query.limit.unwrap_or(PAGE_TEXT).clamp(1, PAGE_TEXT)),
            Some(query.max_links.unwrap_or(LINKS).clamp(0, LINKS)),
        )
        .await?;

    Ok(Json(PageText {
        url: read.url,
        title: read.title,
        text: read.text,
        truncated: read.truncated,
        links: read
            .links
            .into_iter()
            .map(|link| Link {
                text: link.text,
                href: link.href,
            })
            .collect(),
    }))
}

#[derive(Debug, Deserialize)]
struct FindQuery {
    q: String,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    scroll: Option<bool>,
}

/// What on the page matches, best first.
async fn find_elements(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(query): ApiQuery<FindQuery>,
) -> ApiResult<Json<Vec<Element>>> {
    let mut page = visible(&state, &id).await?;
    let found = page
        .find(
            &query.q,
            Some(query.limit.unwrap_or(FOUND).clamp(1, FOUND)),
            query.scroll,
        )
        .await?;

    Ok(Json(found.into_iter().map(element_out).collect()))
}

/// Act on the element a query names.
///
/// By name rather than by coordinate: a point worked out from a frame is stale
/// the moment the page moves under it, and some of these have no coordinate at
/// all — a file chooser is the operating system's window, and a native
/// dropdown opens a menu no screenshot shows.
async fn on_element(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<String>,
    ApiJson(body): ApiJson<OnElement>,
) -> ApiResult<Json<ElementResult>> {
    let mut page = visible(&state, &id).await?;

    Ok(Json(apply(&mut page, body).await?))
}

/// One element operation against a page already in hand.
///
/// Shared with the action batch, so a form is one round trip rather than one
/// per field and the screen lock is held across the whole of it.
async fn apply(page: &mut computer::Page, what: OnElement) -> ApiResult<ElementResult> {
    Ok(match what {
        OnElement::Click { query } => ElementResult {
            element: Some(element_out(page.click_on(&query).await?)),
            ..ElementResult::default()
        },
        OnElement::Fill { query, text } => {
            page.fill(&query, &text).await?;
            ElementResult::default()
        }
        OnElement::Options { query } => ElementResult {
            options: page.options(&query).await?,
            ..ElementResult::default()
        },
        OnElement::Choose { query, option } => {
            page.choose(&query, &option).await?;
            ElementResult::default()
        }
        OnElement::Upload { query, paths } => {
            page.upload(&query, &paths).await?;
            ElementResult::default()
        }
        OnElement::WaitFor {
            query,
            gone,
            within_ms,
        } => {
            let within = Duration::from_millis(within_ms.unwrap_or(WAIT_MS)).min(MAX_WAIT);
            let found = page.wait_for(&query, gone, within).await?;

            ElementResult {
                element: found.map(element_out),
                ..ElementResult::default()
            }
        }
        OnElement::Hover { query } => ElementResult {
            element: Some(element_out(page.hover(&query).await?)),
            ..ElementResult::default()
        },
        OnElement::History { go } => {
            match go {
                Where::Back => page.back().await?,
                Where::Forward => page.forward().await?,
                Where::Reload => page.reload().await?,
            }
            ElementResult::default()
        }
        OnElement::Scroll { query, to, dx, dy } => {
            let how = match to {
                ScrollTo::By => computer::Scroll::By { x: dx, y: dy },
                ScrollTo::Top => computer::Scroll::Top,
                ScrollTo::Bottom => computer::Scroll::Bottom,
            };
            let (x, y) = page.scroll(query.as_deref(), how).await?;

            ElementResult {
                // Never negative: a browser clamps a scroll at the top.
                at: Some(Point {
                    x: x.max(0) as u32,
                    y: y.max(0) as u32,
                }),
                ..ElementResult::default()
            }
        }
    })
}

/// The page the screen is showing.
async fn visible(state: &AppState, id: &str) -> ApiResult<computer::Page> {
    let entry = state.registry.get(id).await?;
    let browser = entry
        .computer
        .browser()
        .ok_or_else(|| ApiError::bad_request("this box publishes no DevTools port"))?;

    browser
        .visible_page()
        .await?
        .ok_or_else(|| ApiError::not_found("no page is on screen"))
}

fn window_out(window: computer::Window) -> Window {
    Window {
        id: window.id,
        title: window.title,
        class: window.class,
        at: Point {
            x: window.at.x,
            y: window.at.y,
        },
        width: window.width,
        height: window.height,
    }
}

fn element_out(element: computer::Element) -> Element {
    Element {
        text: element.text,
        tag: element.tag,
        kind: element.kind,
        at: Point {
            x: element.at.x,
            y: element.at.y,
        },
        width: element.width,
        height: element.height,
        enabled: element.enabled,
        value: element.value,
    }
}

async fn list_windows(
    State(state): State<Arc<AppState>>,
    ApiPath((id, screen)): ApiPath<(String, u32)>,
) -> ApiResult<Json<Vec<Window>>> {
    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;
    let screen = target
        .as_screen()
        .ok_or_else(|| ApiError::bad_request("this screen holds no windows"))?;

    Ok(Json(
        screen
            .windows()
            .await?
            .into_iter()
            .map(window_out)
            .collect(),
    ))
}

/// Untraced: a replay against a fork whose windows opened in another order
/// would raise the wrong one.
async fn focus_window(
    State(state): State<Arc<AppState>>,
    ApiPath((id, screen, window)): ApiPath<(String, u32, String)>,
) -> ApiResult<StatusCode> {
    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;
    let screen = target
        .as_screen()
        .ok_or_else(|| ApiError::bad_request("this screen holds no windows"))?;

    screen.focus(&window).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn close_window(
    State(state): State<Arc<AppState>>,
    ApiPath((id, screen, window)): ApiPath<(String, u32, String)>,
) -> ApiResult<StatusCode> {
    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;
    let screen = target
        .as_screen()
        .ok_or_else(|| ApiError::bad_request("this screen holds no windows"))?;

    screen.close_window(&window).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn arrange_window(
    State(state): State<Arc<AppState>>,
    ApiPath((id, screen, window)): ApiPath<(String, u32, String)>,
    ApiJson(how): ApiJson<Arrange>,
) -> ApiResult<Json<Window>> {
    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;
    let screen = target
        .as_screen()
        .ok_or_else(|| ApiError::bad_request("this screen holds no windows"))?;

    Ok(Json(window_out(
        screen.arrange(&window, arranging(how)).await?,
    )))
}

async fn active_window(
    State(state): State<Arc<AppState>>,
    ApiPath((id, screen)): ApiPath<(String, u32)>,
) -> ApiResult<Json<Option<Window>>> {
    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;
    let screen = target
        .as_screen()
        .ok_or_else(|| ApiError::bad_request("this screen holds no windows"))?;

    Ok(Json(screen.active_window().await?.map(window_out)))
}

async fn await_window(
    State(state): State<Arc<AppState>>,
    ApiPath((id, screen)): ApiPath<(String, u32)>,
    ApiJson(body): ApiJson<AwaitWindow>,
) -> ApiResult<Json<Window>> {
    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;
    let screen = target
        .as_screen()
        .ok_or_else(|| ApiError::bad_request("this screen holds no windows"))?;

    let within = Duration::from_millis(body.within_ms.unwrap_or(computer::apps::READY_MS));

    Ok(Json(window_out(
        screen.wait_for_window(&body.class, within).await?,
    )))
}

fn arranging(how: Arrange) -> computer::Arrange {
    match how {
        Arrange::At { to } => computer::Arrange::At(computer::Point::new(to.x, to.y)),
        Arrange::Size { width, height } => computer::Arrange::Size { width, height },
        Arrange::Maximise => computer::Arrange::Maximise,
        Arrange::Minimise => computer::Arrange::Minimise,
        Arrange::Restore => computer::Arrange::Restore,
    }
}

/// So an agent reads the names rather than guessing one and meeting a 400.
async fn catalog() -> Json<BTreeMap<String, computer_types::App>> {
    Json(computer::apps::builtin())
}

/// The builder, pointed at whatever this server reaches runtimes through.
fn through(builder: computer::Builder, state: &AppState) -> computer::Builder {
    match &state.cli {
        Some(cli) => builder.cli(Arc::clone(cli)),
        None => builder,
    }
}

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// One request's claim on an idempotency key.
///
/// The key is bound to the route and the body it first arrived on. A retry
/// carries both again and is answered from the store; the same key on a
/// different request is a client bug, and returning the first request's reply
/// would hide it behind a success.
struct Idempotent {
    key: Option<String>,
    print: idempotency::Fingerprint,
}

impl Idempotent {
    fn of<T: serde::Serialize>(headers: &HeaderMap, route: &str, body: &T) -> Self {
        let bytes = serde_json::to_vec(body).unwrap_or_default();

        Self {
            key: header(headers, IDEMPOTENCY_KEY),
            print: idempotency::fingerprint(route, &bytes),
        }
    }

    /// The reply this request was already given, if it is the same request.
    fn replay(&self, replies: &Replies) -> ApiResult<Option<Response>> {
        let Some(key) = self.key.as_deref() else {
            return Ok(None);
        };

        match replies.lookup(key, self.print) {
            Lookup::Fresh => Ok(None),
            Lookup::Replay { status, body } => {
                let status = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
                Ok(Some(json_response(status, body)))
            }
            Lookup::Reused => Err(ApiError::new(
                StatusCode::CONFLICT,
                ErrorCode::Denied,
                format!(
                    "idempotency-key {key} was used for a different request; \
                     answering this one with the other's reply would report \
                     work that never happened"
                ),
            )),
        }
    }

    /// Keeps the answer where a retry of the same request will find it.
    fn answer<T: serde::Serialize>(
        &self,
        replies: &Replies,
        status: StatusCode,
        value: &T,
    ) -> ApiResult<Response> {
        let body = serde_json::to_vec(value).map_err(|error| {
            ApiError::internal(format!("the answer would not serialise: {error}"))
        })?;

        if let Some(key) = self.key.as_deref() {
            replies.put(key, self.print, status.as_u16(), body.clone());
        }

        Ok(json_response(status, body))
    }
}

fn json_response(status: StatusCode, body: Vec<u8>) -> Response {
    (
        status,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response()
}

fn view_of(entry: &Entry) -> BoxView {
    BoxView {
        id: entry.id.clone(),
        spec_digest: entry.spec_digest(),
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

fn button_in(button: Button) -> EngineButton {
    match button {
        Button::Left => EngineButton::Left,
        Button::Right => EngineButton::Right,
        Button::Middle => EngineButton::Middle,
    }
}

fn selection_in(selection: Selection) -> EngineSelection {
    match selection {
        Selection::Clipboard => EngineSelection::Clipboard,
        Selection::Primary => EngineSelection::Primary,
    }
}
