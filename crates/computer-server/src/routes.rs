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
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use computer::{Delta, Desktop as EngineDesktop};
use computer_api::*;
use computer_storage::BoxRecord;
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
        .route("/v1/boxes/{id}/pages", get(list_tabs))
        .route("/v1/boxes/{id}/pages/{tab}", delete(close_tab))
        .route("/v1/boxes/{id}/pages/{tab}/focus", post(focus_tab))
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

    kept(&state, &entry.id, &body.spec, &body.placement, &resolved).await;
    state
        .record(
            &entry.id,
            Actor::Agent,
            TraceEvent::BoxCreated {
                spec_digest: entry.spec_digest(),
                spec: Box::new(body.spec.clone()),
                placement: Box::new(body.placement.clone()),
                width: resolved.width,
                height: resolved.height,
                screens: resolved.screens,
            },
        )
        .await;

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
        .record(&id, Actor::Agent, TraceEvent::BoxDeleted)
        .await;
    state.forget_screens(&id);

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
    let mut tabs = Vec::new();
    let mut stopped_at = None;

    for (index, action) in batch.actions.iter().enumerate() {
        let outcome = run(
            desktop,
            target.as_screen(),
            action,
            &entry.spec,
            browser.as_ref(),
            &mut page,
            &mut tabs,
        )
        .await;

        // Its own event: a name and its arguments are the whole launch.
        match (&outcome, action) {
            (Ok(Some(window)), Action::Launch { app, args }) => {
                state
                    .record(
                        &id,
                        Actor::Agent,
                        TraceEvent::AppLaunched {
                            screen,
                            app: app.clone(),
                            args: args.clone(),
                            window: window.id.clone(),
                        },
                    )
                    .await
            }
            _ => {
                state
                    .record(
                        &id,
                        Actor::Agent,
                        TraceEvent::Acted {
                            screen,
                            action: action.clone(),
                            ok: outcome.is_ok(),
                            error: outcome.as_ref().err().map(|error| error.body.clone()),
                        },
                    )
                    .await
            }
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
                &state,
                &id,
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
        desktop.cursor().await.ok()
    } else {
        None
    };

    stamp.answer(
        &state.replies,
        StatusCode::OK,
        &BatchResult {
            results,
            windows,
            tabs,
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
    tabs: &mut Vec<Tab>,
) -> ApiResult<Option<Window>> {
    match action {
        Action::Move { to } => desktop.move_to(*to).await?,
        Action::Click { at, button, held } => {
            let at = match at {
                Some(at) => *at,
                None => desktop.cursor().await?,
            };
            desktop.click_with(at, *button, held).await?;
        }
        Action::DoubleClick { at, button } => {
            let at = match at {
                Some(at) => *at,
                None => desktop.cursor().await?,
            };
            desktop.double_click(at, *button).await?;
        }
        Action::Drag {
            from,
            to,
            button,
            held,
        } => desktop.drag_with(*from, *to, *button, held).await?,
        Action::Type { text } => desktop.type_text(text).await?,
        Action::Key { chord } => desktop.key(chord).await?,
        Action::Scroll { at, dx, dy } => desktop.scroll(*at, Delta { dx: *dx, dy: *dy }).await?,
        Action::OpenUrl { url, target } => {
            // Whatever page action follows wants the page this leaves on
            // screen, not the one the batch started on.
            *page = None;

            match (browser, target) {
                // Through the debugger, which is the only way to learn the id
                // of what was opened and the only way to reuse a tab.
                (Some(browser), OpenIn::Blank) => {
                    let opened = browser.open(url).await?;
                    let mut fresh = browser.attach(&opened).await?;
                    // `PUT /json/new` does not raise it.
                    fresh.bring_to_front().await?;

                    tabs.push(tab_out(&opened, true));

                    // And the ones before it do not accumulate. Best effort: a
                    // browser that would not say what it holds is not a reason
                    // to refuse the page that just opened.
                    let _ = browser.tidy(TABS).await;
                }
                (Some(browser), OpenIn::Current) => {
                    let mut showing = match browser.visible_page().await? {
                        Some(showing) => showing,
                        None => browser.first_page().await?,
                    };
                    showing.navigate(url).await?;

                    tabs.push(tab_out(showing.target(), true));
                }
                // No debugger to reach, so the browser in the box opens it and
                // nothing here learns its id. `current` cannot be honoured at
                // all: there is no page to navigate.
                (None, _) => {
                    let screen = screen.ok_or_else(|| {
                        ApiError::bad_request("this screen has no browser to open a page in")
                    })?;
                    screen.open_url(url).await?;
                }
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

            // No settle of its own: the batch takes one frame after all of its
            // actions, and pausing inside each would pay it per action.
            apply(page, what.clone(), Duration::ZERO).await?;
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

            return Ok(Some(window));
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
            (Some(x), Some(y), Some(width), Some(height)) => Some(Rect {
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

    Ok(Json(
        recorded(
            &state,
            &id,
            Actor::Agent,
            screen,
            png,
            query.have.as_deref(),
        )
        .await,
    ))
}

fn shot_in(shot: &Shot) -> computer::Shot {
    let of = match (&shot.window, &shot.region) {
        (Some(window), _) => computer::Of::Window(window.clone()),
        (None, Some(area)) => {
            computer::Of::Region(computer::Rect::new(area.at, area.width, area.height))
        }
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
    state: &AppState,
    id: &str,
    actor: Actor,
    screen: u32,
    desktop: &dyn EngineDesktop,
    have: Option<&str>,
) -> ApiResult<Frame> {
    let png = desktop.screenshot().await?;

    Ok(recorded(state, id, actor, screen, png, have).await)
}

/// Left out altogether when the caller already holds it.
async fn recorded(
    state: &AppState,
    id: &str,
    actor: Actor,
    screen: u32,
    png: Vec<u8>,
    have: Option<&str>,
) -> Frame {
    let mut hasher = Sha256::new();
    hasher.update(&png);
    let hash = format!("{:x}", hasher.finalize());

    state.note_frame(id, actor, screen, &hash, &png).await;

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

    Ok(Json(target.as_desktop().cursor().await?))
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

    let text = held.selection(query.selection).await?;

    state
        .record(
            &id,
            Actor::Agent,
            TraceEvent::ClipboardRead {
                screen,
                selection: query.selection,
            },
        )
        .await;

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

    held.set_selection(body.selection, &body.text).await?;

    state
        .record(
            &id,
            Actor::Agent,
            TraceEvent::ClipboardSet {
                screen,
                selection: body.selection,
            },
        )
        .await;

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

    // The frame the person is being given, so what they changed is the
    // difference between this and the one taken when they hand it back.
    let _ = capture(&state, &id, Actor::Agent, screen, target.as_desktop(), None).await;

    let takeover = if body.shared {
        held.share().await?
    } else {
        held.hand_over().await?
    };

    state
        .record(
            &id,
            Actor::Agent,
            TraceEvent::TakeoverStarted {
                screen,
                exclusive: takeover.exclusive(),
            },
        )
        .await;

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

    // Taken while the screen is still theirs — reading is allowed during a
    // handover — so the frame is what the person left rather than what
    // happened after they let go.
    let _ = capture(
        &state,
        &id,
        Actor::Person,
        screen,
        target.as_desktop(),
        None,
    )
    .await;

    held.reclaim().await?;
    state
        .record(&id, Actor::Agent, TraceEvent::TakeoverEnded { screen })
        .await;

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

    state
        .record(
            &id,
            Actor::Agent,
            TraceEvent::Executed {
                argv: body.argv.clone(),
                code: result.code,
                timed_out: result.timed_out,
            },
        )
        .await;

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

    state
        .record(
            &id,
            Actor::Agent,
            TraceEvent::FileRead {
                path: query.path.clone(),
                bytes: bytes.len(),
            },
        )
        .await;

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

    state
        .record(
            &id,
            Actor::Agent,
            TraceEvent::FileWritten {
                path: body.path.clone(),
                bytes: bytes.len(),
            },
        )
        .await;

    Ok(StatusCode::NO_CONTENT)
}

/// Build a box again from what was done to the first one.
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

    let history = state.store.entries(&id, None, usize::MAX).await?;
    if history.is_empty() {
        return Err(ApiError::not_found(format!(
            "nothing was ever traced for {id}"
        )));
    }
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

    kept(&state, &new_id, &spec, &placement, &resolved).await;
    state
        .record(
            &new_id,
            Actor::Agent,
            TraceEvent::BoxCreated {
                spec_digest: entry.spec_digest(),
                spec,
                placement,
                width: resolved.width,
                height: resolved.height,
                screens: resolved.screens,
            },
        )
        .await;
    state
        .record(
            &new_id,
            Actor::Agent,
            TraceEvent::ForkedFrom {
                source: id.clone(),
                up_to: body.up_to,
            },
        )
        .await;

    let report = replay_onto(&entry, &state, &new_id, &history, body.up_to).await;

    // Close with what the replay produced, so a caller can compare it against
    // the source's own last frame. They will rarely be the same bytes: a
    // desktop animates, which is why the report counts actions rather than
    // claiming the two boxes match.
    if let Ok(target) = entry.desktop(0).await {
        let _ = capture(&state, &new_id, Actor::Agent, 0, target.as_desktop(), None).await;
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
    state: &AppState,
    id: &str,
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
                            // A replay drives a fresh box; the ids a tab had in
                            // the source mean nothing in it.
                            &mut Vec::new(),
                        )
                        .await
                    }
                    Err(error) => Err(error),
                };

                match (&acted, action) {
                    (Ok(Some(window)), Action::Launch { app, args }) => {
                        state
                            .record(
                                id,
                                Actor::Agent,
                                TraceEvent::AppLaunched {
                                    screen: *screen,
                                    app: app.clone(),
                                    args: args.clone(),
                                    window: window.id.clone(),
                                },
                            )
                            .await
                    }
                    _ => {
                        state
                            .record(
                                id,
                                Actor::Agent,
                                TraceEvent::Acted {
                                    screen: *screen,
                                    action: action.clone(),
                                    ok: acted.is_ok(),
                                    error: acted.as_ref().err().map(|error| error.body.clone()),
                                },
                            )
                            .await
                    }
                };
                acted.map(|_| ())
            }
            Step::Exec { argv } => {
                let ran = entry.computer.exec(argv).await.map_err(ApiError::from);

                if let Ok(result) = &ran {
                    state
                        .record(
                            id,
                            Actor::Agent,
                            TraceEvent::Executed {
                                argv: argv.clone(),
                                code: result.code,
                                timed_out: result.timed_out,
                            },
                        )
                        .await;
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
    let limit = query.limit.unwrap_or(TRACE_PAGE).clamp(1, TRACE_PAGE);
    let entries = state.store.entries(&id, query.after, limit).await?;

    if entries.is_empty() && !state.traced(&id).await {
        return Err(ApiError::not_found(format!(
            "nothing was ever traced for {id}"
        )));
    }

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
    let png = state.frames.get(&id, &hash).await?.ok_or_else(|| {
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
    #[serde(default)]
    tab: Option<String>,
}

/// What the page in front is showing, as text.
/// The page the screen shows, not the first one open: a caller reading what it
/// can see is the point, and a frame and this have to agree.
async fn read_page(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(query): ApiQuery<PageQuery>,
) -> ApiResult<Json<PageText>> {
    let mut page = page_for(&state, &id, query.tab.as_deref()).await?;

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
    #[serde(default)]
    exact: Option<bool>,
    #[serde(default)]
    tab: Option<String>,
}

/// What on the page matches, best first.
async fn find_elements(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(query): ApiQuery<FindQuery>,
) -> ApiResult<Json<Vec<Element>>> {
    let mut page = page_for(&state, &id, query.tab.as_deref()).await?;
    let found = page
        .find(
            &query.q,
            Some(query.limit.unwrap_or(FOUND).clamp(1, FOUND)),
            query.scroll,
            query.exact,
        )
        .await?;

    Ok(Json(found.into_iter().map(element_out).collect()))
}

/// Act on the element a query names.
/// By name rather than by coordinate: a point worked out from a frame is stale
/// the moment the page moves under it, and some of these have no coordinate at
/// all — a file chooser is the operating system's window, and a native
/// dropdown opens a menu no screenshot shows.
async fn on_element(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(query): ApiQuery<SettleQuery>,
    ApiJson(body): ApiJson<OnElement>,
) -> ApiResult<Json<ElementResult>> {
    let mut page = page_for(&state, &id, query.tab.as_deref()).await?;
    let settle = Duration::from_millis(query.settle_ms.unwrap_or_default()).min(MAX_PAUSE);

    Ok(Json(apply(&mut page, body, settle).await?))
}

/// How long to let the page stop moving before it is measured.
///
/// A click that opens a menu or starts a navigation has not finished when the
/// call returns, and a URL or a frame read at that moment is the page on its
/// way rather than the page it arrived at.
#[derive(Debug, Deserialize)]
struct SettleQuery {
    #[serde(default)]
    settle_ms: Option<u64>,
    #[serde(default)]
    tab: Option<String>,
}

/// One element operation against a page already in hand.
/// Shared with the action batch, so a form is one round trip rather than one
/// per field and the screen lock is held across the whole of it.
async fn apply(
    page: &mut computer::Page,
    what: OnElement,
    settle: Duration,
) -> ApiResult<ElementResult> {
    let before = page.url().await.ok();
    let mut result = applied(page, what).await?;

    if !settle.is_zero() {
        tokio::time::sleep(settle).await;
    }

    if let Ok(after) = page.url().await {
        result.navigated = before.as_deref() != Some(after.as_str());
        result.url = Some(after);
    }

    Ok(result)
}

async fn applied(page: &mut computer::Page, what: OnElement) -> ApiResult<ElementResult> {
    Ok(match what {
        OnElement::Click { query, button } => ElementResult {
            element: Some(element_out(page.click_on(&query, button).await?)),
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
            or,
            exact,
        } => {
            let within = Duration::from_millis(within_ms.unwrap_or(WAIT_MS)).min(MAX_WAIT);
            let (matched, found) = page.wait_for_any(&query, &or, gone, within, exact).await?;

            ElementResult {
                element: found.map(element_out),
                matched: Some(matched),
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

/// The pages a box has open.
async fn list_tabs(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<Json<Vec<Tab>>> {
    let browser = debugger(&state, &id).await?;

    // Which one is frontmost costs an attach each: the debugger lists tabs but
    // never says which the person is looking at.
    let showing = match browser.visible_page().await {
        Ok(Some(page)) => Some(page.target().id.clone()),
        _ => None,
    };

    let tabs = browser
        .pages()
        .await?
        .iter()
        .map(|target| tab_out(target, showing.as_deref() == Some(target.id.as_str())))
        .collect();

    Ok(Json(tabs))
}

async fn focus_tab(
    State(state): State<Arc<AppState>>,
    ApiPath((id, tab)): ApiPath<(String, String)>,
) -> ApiResult<StatusCode> {
    named(&state, &id, &tab).await?.bring_to_front().await?;

    Ok(StatusCode::NO_CONTENT)
}

async fn close_tab(
    State(state): State<Arc<AppState>>,
    ApiPath((id, tab)): ApiPath<(String, String)>,
) -> ApiResult<StatusCode> {
    debugger(&state, &id).await?.close(&tab).await?;

    Ok(StatusCode::NO_CONTENT)
}

/// This box's debugger.
async fn debugger(state: &AppState, id: &str) -> ApiResult<computer::Devtools> {
    let entry = state.registry.get(id).await?;

    entry
        .computer
        .browser()
        .ok_or_else(|| ApiError::bad_request("this box publishes no DevTools port"))
}

/// The page a request names, or the one on screen where it names none.
///
/// Naming one is also cheaper: finding the visible page means attaching to every
/// tab and asking each whether it is frontmost.
async fn page_for(state: &AppState, id: &str, tab: Option<&str>) -> ApiResult<computer::Page> {
    match tab {
        Some(tab) => named(state, id, tab).await,
        None => visible(state, id).await,
    }
}

async fn named(state: &AppState, id: &str, tab: &str) -> ApiResult<computer::Page> {
    let browser = debugger(state, id).await?;

    let target = browser
        .pages()
        .await?
        .into_iter()
        .find(|target| target.id == tab)
        .ok_or_else(|| ApiError::not_found(format!("this box has no tab {tab}")))?;

    Ok(browser.attach(&target).await?)
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

fn tab_out(target: &computer::cdp::Target, visible: bool) -> Tab {
    Tab {
        id: target.id.clone(),
        title: target.title.clone(),
        url: target.url.clone(),
        visible,
    }
}

fn element_out(element: computer::Element) -> Element {
    Element {
        text: element.text,
        tag: element.tag,
        kind: element.kind,
        selector: element.selector,
        label: element.label,
        visible: element.visible,
        at: element.at.map(|at| Point { x: at.x, y: at.y }),
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

    Ok(Json(screen.windows().await?.into_iter().collect()))
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

    Ok(Json(screen.arrange(&window, how).await?))
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

    Ok(Json(screen.active_window().await?))
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

    Ok(Json(screen.wait_for_window(&body.class, within).await?))
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

async fn kept(
    state: &AppState,
    id: &str,
    spec: &Spec,
    placement: &Placement,
    resolved: &spec::Resolved,
) -> BoxRecord {
    let record = BoxRecord {
        id: id.to_string(),
        spec: spec.clone(),
        placement: placement.clone(),
        width: resolved.width,
        height: resolved.height,
        screens: resolved.screens,
        created_at_ms: millis(SystemTime::now()),
        expires_at_ms: None,
    };

    if let Err(why) = state.store.put_box(&record).await {
        tracing::warn!(box_ = %id, %why, "a box was not recorded");
    }

    record
}

pub(crate) fn ms_of(at: SystemTime) -> u64 {
    millis(at)
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
