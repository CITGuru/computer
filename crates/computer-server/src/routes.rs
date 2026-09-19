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
use computer::motion::path;
use computer::{Delta, Desktop as EngineDesktop};
use computer_api::*;
use computer_storage::BoxRecord;
use computer_types::{Search, Spec};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::collections::btree_map::Entry as Entry_;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const HEALTH: &str = "/v1/health";
const IDEMPOTENCY_KEY: &str = "idempotency-key";
const CONFIRM_DELETE: &str = "x-computer-confirm-delete";
const TRACE_PAGE: usize = 500;
const RAISE: Duration = Duration::from_millis(250);
const MAX_PACE: Duration = Duration::from_millis(1_000);
const WAKE: Duration = Duration::from_secs(90);
const REPLAY_BUDGET: Duration = Duration::from_secs(180);
const PAGE_TEXT: usize = 20_000;
const EVALUATED: usize = 100_000;
/// Capped: the screen lock is held across an evaluation.
const EVALUATE_MS: u64 = 5_000;
const LINKS: usize = 100;
const FOUND: usize = 50;
const SNAPSHOT: usize = 300;
/// Enough to act on; a bigger change is a page to snapshot again.
const DELTA_LINES: usize = 12;
const TABS: usize = 12;
const WAIT_MS: u64 = 10_000;
const MAX_WAIT: Duration = Duration::from_secs(60);
const REPLAY_GAP_CAP: Duration = Duration::from_secs(2);
/// The screen lock is held across a pause, so none may be uncapped.
const MAX_PAUSE: Duration = Duration::from_secs(30);

const SETTLE: u64 = 400;
const STILL: u64 = 10_000;
const MAX_EXEC: Duration = Duration::from_secs(600);

pub fn router(state: Arc<AppState>) -> Router {
    // A browser opens a WebSocket with no header to carry a bearer in, so the viewer
    // socket sits outside the gate and admits a ticket instead.
    let open = Router::new()
        .route(
            "/v1/boxes/{id}/screens/{screen}/viewer/socket",
            get(crate::viewer::socket),
        )
        .with_state(Arc::clone(&state));

    let gated = Router::new()
        .route(HEALTH, get(health))
        .route("/v1/boxes", get(list_boxes).post(create_box))
        .route("/v1/boxes/{id}", get(get_box).delete(delete_box))
        .route("/v1/boxes/{id}/fork", post(fork))
        .route("/v1/boxes/{id}/pause", post(pause_box))
        .route("/v1/boxes/{id}/resume", post(resume_box))
        .route("/v1/boxes/{id}/stop", post(stop_box))
        .route("/v1/boxes/{id}/exec", post(exec))
        .route("/v1/boxes/{id}/trace", get(read_trace))
        .route("/v1/boxes/{id}/trace/frames/{hash}", get(trace_frame))
        .route("/v1/boxes/{id}/files", get(read_file).put(write_file))
        .route("/v1/boxes/{id}/files/list", get(list_dir))
        .route("/v1/boxes/{id}/files/grep", post(grep))
        .route("/v1/boxes/{id}/files/glob", get(glob))
        .route("/v1/boxes/{id}/screens/{screen}/actions", post(actions))
        .route("/v1/boxes/{id}/screens/{screen}/frame", get(frame))
        .route("/v1/boxes/{id}/screens/{screen}/cursor", get(cursor))
        .route(
            "/v1/boxes/{id}/screens/{screen}/desktop/node",
            post(on_node),
        )
        .route(
            "/v1/boxes/{id}/screens/{screen}/clipboard",
            get(get_clipboard).put(set_clipboard),
        )
        .route(
            "/v1/boxes/{id}/screens/{screen}/takeover",
            post(start_takeover).delete(end_takeover),
        )
        .route("/v1/boxes/{id}/screens/{screen}/viewers", get(viewers))
        .route(
            "/v1/boxes/{id}/screens/{screen}/viewer/ticket",
            post(crate::viewer::ticket),
        )
        .route(
            "/v1/boxes/{id}/screens/{screen}/recording",
            get(recording).post(start_recording).delete(stop_recording),
        )
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
        .route("/v1/boxes/{id}/page/snapshot", get(snapshot_page))
        .route("/v1/boxes/{id}/page/element", post(on_element))
        .route("/v1/boxes/{id}/page/evaluate", post(evaluate))
        .route("/v1/boxes/{id}/page/screenshot", post(page_screenshot))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&state),
            crate::auth::gate,
        ))
        .with_state(state);

    open.merge(gated)
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
    let mut boxes = Vec::new();

    for entry in state.registry.list().await.iter() {
        boxes.push(viewed(entry, state_of(entry).await));
    }

    Json(BoxList { boxes })
}

async fn state_of(entry: &Entry) -> BoxState {
    // Stopped first: a stopped container cannot be paused, and reports not paused.
    if let Ok(true) = entry.computer.stopped().await {
        return BoxState::Stopped;
    }

    match entry.computer.paused().await {
        Ok(true) => BoxState::Paused,
        _ => BoxState::Ready,
    }
}

async fn get_box(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<Json<BoxView>> {
    let entry = state.registry.get(&id).await?;
    let state = state_of(&entry).await;

    Ok(Json(viewed(&entry, state)))
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
    state.tickets.forget(&id);

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

    // Resolved lazily: `open_url` raises a new tab, so an early handle is stale.
    let browser = entry.computer.browser();
    let mut page = None;

    let mut results = Vec::with_capacity(batch.actions.len());
    let mut windows = Vec::new();
    let mut tabs = Vec::new();
    let mut stopped_at = None;

    for (index, action) in batch.actions.iter().enumerate() {
        let outcome = run(
            &mut Doing {
                state: &state,
                id: &id,
                number: screen,
                entry: &entry,
                desktop,
                screen: target.as_screen(),
                spec: &entry.spec,
                browser: browser.as_ref(),
                page: &mut page,
                tabs: &mut tabs,
            },
            action,
        )
        .await;

        match (&outcome, action) {
            (
                Ok(Did {
                    window: Some(window),
                    ..
                }),
                Action::Launch { app, args },
            ) => {
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
            Ok(did) => {
                windows.extend(did.window);
                results.push(ActionResult {
                    index,
                    ok: true,
                    error: None,
                    out: did.out,
                })
            }
            Err(error) => {
                // Stop: a click after a failed move lands wherever the pointer was.
                results.push(ActionResult {
                    index,
                    ok: false,
                    error: Some(error.body),
                    out: None,
                });

                // Only where it did stop: with `keep_going` the step was
                // refused and the rest still ran.
                if !batch.keep_going {
                    stopped_at = Some(index);
                    break;
                }
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

/// What one step leaves behind: a window a launch drew, and what a read saw.
#[derive(Default)]
struct Did {
    window: Option<Window>,
    out: Option<Out>,
}

impl Did {
    fn read(out: Out) -> Self {
        Self {
            window: None,
            out: Some(out),
        }
    }
}

/// Everything a step may reach. A batch holds the screen for its whole length,
/// so a step that goes outside the screen still belongs to the same box.
struct Doing<'a> {
    state: &'a AppState,
    id: &'a str,
    number: u32,
    entry: &'a Entry,
    desktop: &'a dyn EngineDesktop,
    screen: Option<&'a computer::Screen>,
    spec: &'a Spec,
    browser: Option<&'a computer::Devtools>,
    page: &'a mut Option<computer::Page>,
    tabs: &'a mut Vec<Tab>,
}

impl Doing<'_> {
    fn held(&self, cannot: &str) -> ApiResult<&computer::Screen> {
        self.screen
            .ok_or_else(|| ApiError::bad_request(format!("this screen {cannot}")))
    }
}

/// The steps that read, and the ones that reach past the screen. Each answers
/// with what its own endpoint would.
async fn reaching(doing: &mut Doing<'_>, action: &Action) -> ApiResult<Did> {
    let out = match action {
        Action::Evaluate { what } => {
            let mut page = page_for(doing.state, doing.id, None).await?;
            let within =
                Duration::from_millis(what.timeout_ms.unwrap_or(EVALUATE_MS)).min(MAX_PAUSE);
            let value = page.evaluate_within(&what.expression, Some(within)).await?;

            let json = serde_json::to_string(&value).map_err(|error| {
                ApiError::internal(format!("the answer would not serialise: {error}"))
            })?;
            let limit = what.limit.unwrap_or(EVALUATED).clamp(1, EVALUATED);

            Out::Value(Evaluated {
                truncated: json.chars().count() > limit,
                json: json.chars().take(limit).collect(),
            })
        }
        Action::Look { what } => {
            let mut page = page_for(doing.state, doing.id, what.tab.as_deref()).await?;
            let query = match what.role.as_deref() {
                Some(role) => computer::cdp::selector_for(role)
                    .ok_or_else(|| {
                        ApiError::bad_request(format!(
                            "no such role: {role}. This server knows {}",
                            computer::cdp::ROLES
                        ))
                    })?
                    .to_string(),
                None => what.query.clone(),
            };

            let found = page
                .find(
                    &query,
                    Some(what.limit.unwrap_or(FOUND).clamp(1, FOUND)),
                    what.scroll,
                    what.exact,
                )
                .await?;

            Out::Elements(found.into_iter().map(element_out).collect())
        }
        Action::Snapshot { what } => {
            let mut page = page_for(doing.state, doing.id, what.tab.as_deref()).await?;

            if let Some(quiet) = what.quiet_ms {
                let quiet = Duration::from_millis(quiet).min(MAX_WAIT);
                page.quiet(quiet, Duration::from_millis(WAIT_MS).max(quiet))
                    .await?;
            }

            let limit = Some(what.limit.unwrap_or(SNAPSHOT).clamp(1, SNAPSHOT));
            let taken = match what.delta {
                true => page.snapshot_delta(what.scope.as_deref(), limit).await?,
                false => page.snapshot(what.scope.as_deref(), limit).await?,
            };

            Out::Snapshot(Box::new(snapshot_out(taken)))
        }
        Action::Read { what } => {
            let mut page = page_for(doing.state, doing.id, what.tab.as_deref()).await?;
            Out::Text(Box::new(read_out(&mut page, what).await?))
        }
        Action::PageShot { what } => {
            Out::Picture(captured_page(doing.state, doing.id, what).await?)
        }
        Action::Capture { what } => {
            if let Some(tab) = &what.tab {
                named(doing.state, doing.id, tab)
                    .await?
                    .bring_to_front()
                    .await?;
                tokio::time::sleep(RAISE).await;
            }

            let png = match what.is_whole() {
                true => doing.desktop.screenshot().await?,
                false => {
                    doing
                        .held("cannot be captured in part")?
                        .capture(&shot_in(what))
                        .await?
                }
            };

            Out::Frame(recorded(doing.state, doing.id, Actor::Agent, doing.number, png, None).await)
        }
        Action::Cursor => Out::At(doing.desktop.cursor().await?),
        Action::Windows { active } => {
            let held = doing.held("holds no windows")?;

            match active {
                true => Out::Window(held.active_window().await?),
                false => Out::Windows(held.windows().await?.into_iter().collect()),
            }
        }
        Action::AwaitWindow { what } => {
            let held = doing.held("holds no windows")?;
            let within = Duration::from_millis(what.within_ms.unwrap_or(computer::apps::READY_MS));

            Out::Window(Some(held.wait_for_window(&what.class, within).await?))
        }
        Action::OnWindow { window, what } => {
            let held = doing.held("holds no windows")?;

            match what {
                WindowOp::Focus => held.focus(window).await?,
                WindowOp::Close => held.close_window(window).await?,
                WindowOp::Arrange { how } => {
                    return Ok(Did::read(Out::Window(Some(
                        held.arrange(window, *how).await?,
                    ))));
                }
            }

            Out::Window(held.active_window().await?)
        }
        Action::Tabs => Out::Tabs(listed_tabs(doing.state, doing.id).await?),
        Action::OnTab { tab, close } => {
            match close {
                true => debugger(doing.state, doing.id).await?.close(tab).await?,
                false => {
                    named(doing.state, doing.id, tab)
                        .await?
                        .bring_to_front()
                        .await?
                }
            }

            Out::Tabs(listed_tabs(doing.state, doing.id).await?)
        }
        Action::Exec { what } => {
            if what.argv.is_empty() {
                return Err(ApiError::bad_request("argv is empty"));
            }

            let ran = match what.timeout_ms {
                Some(ms) => {
                    doing
                        .entry
                        .computer
                        .exec_within(&what.argv, Duration::from_millis(ms).min(MAX_EXEC))
                        .await?
                }
                None => doing.entry.computer.exec(&what.argv).await?,
            };

            doing
                .state
                .record(
                    doing.id,
                    Actor::Agent,
                    TraceEvent::Executed {
                        argv: what.argv.clone(),
                        code: ran.code,
                        timed_out: ran.timed_out,
                    },
                )
                .await;

            Out::Ran(ExecResponse {
                code: ran.code,
                stdout: ran.stdout_utf8(),
                stderr: ran.stderr_utf8(),
                timed_out: ran.timed_out,
            })
        }
        Action::ReadFile { path } => {
            let bytes = doing.entry.computer.read_file(path).await?;

            doing
                .state
                .record(
                    doing.id,
                    Actor::Agent,
                    TraceEvent::FileRead {
                        path: path.clone(),
                        bytes: bytes.len(),
                    },
                )
                .await;

            Out::File(ReadFile {
                path: path.clone(),
                contents_base64: BASE64.encode(bytes),
            })
        }
        Action::WriteFile { what } => {
            let bytes = BASE64
                .decode(what.contents_base64.as_bytes())
                .map_err(|error| {
                    ApiError::bad_request(format!("contents_base64 is not base64: {error}"))
                })?;

            doing.entry.computer.write_file(&what.path, &bytes).await?;
            doing
                .state
                .record(
                    doing.id,
                    Actor::Agent,
                    TraceEvent::FileWritten {
                        path: what.path.clone(),
                        bytes: bytes.len(),
                    },
                )
                .await;

            return Ok(Did::default());
        }
        Action::Clipboard { selection, text } => {
            let held = doing.held("has no clipboard")?;

            match text {
                Some(text) => {
                    held.set_selection(*selection, text).await?;
                    return Ok(Did::default());
                }
                None => Out::Clipboard(ClipboardView {
                    text: held.selection(*selection).await?,
                }),
            }
        }
        Action::Record { what } => {
            let held = doing.held("cannot be recorded")?;

            let (recording, path) = match what {
                RecordOp::Start { fps } => {
                    if let Some(fps) = fps
                        && !(1..=60).contains(fps)
                    {
                        return Err(ApiError::bad_request("fps must be between 1 and 60"));
                    }

                    (true, Some(held.start_recording(*fps).await?))
                }
                RecordOp::Stop => (false, Some(held.stop_recording().await?)),
                RecordOp::Status => {
                    let path = held.recording().await?;
                    (path.is_some(), path)
                }
            };

            Out::Recording(RecordingView { recording, path })
        }
        Action::Apps => Out::Apps(computer::apps::builtin().keys().cloned().collect()),
        _ => return Err(ApiError::internal("this step has no runner")),
    };

    Ok(Did::read(out))
}

async fn run(doing: &mut Doing<'_>, action: &Action) -> ApiResult<Did> {
    let desktop = doing.desktop;
    let screen = doing.screen;
    let spec = doing.spec;
    let browser = doing.browser;

    match action {
        Action::Move { to, motion, seed } => match motion.is_instant() {
            true => desktop.move_to(*to).await?,
            false => {
                let from = pointer_start(desktop, spec).await;
                desktop
                    .move_along(&path(from, *to, *motion, seed.unwrap_or(0)))
                    .await?
            }
        },
        Action::Click {
            at,
            button,
            held,
            motion,
            seed,
        } => {
            let at = match at {
                Some(at) => *at,
                None => desktop.cursor().await?,
            };
            approach(desktop, spec, at, *motion, *seed).await?;
            desktop.click_with(at, *button, held).await?;
        }
        Action::DoubleClick {
            at,
            button,
            motion,
            seed,
        } => {
            let at = match at {
                Some(at) => *at,
                None => desktop.cursor().await?,
            };
            approach(desktop, spec, at, *motion, *seed).await?;
            desktop.double_click(at, *button).await?;
        }
        Action::Drag {
            from,
            to,
            button,
            held,
            motion,
            seed,
        } => match motion.is_instant() {
            true => desktop.drag_with(*from, *to, *button, held).await?,
            false => {
                let seed = seed.unwrap_or(0);
                approach(desktop, spec, *from, *motion, Some(seed)).await?;
                desktop
                    .drag_along(
                        *from,
                        &path(*from, *to, *motion, seed.wrapping_add(1)),
                        *button,
                        held,
                    )
                    .await?
            }
        },
        Action::Path {
            through,
            button,
            held,
            motion,
            seed,
        } => {
            let [first, rest @ ..] = through.as_slice() else {
                return Err(ApiError::bad_request("a path needs at least one point"));
            };
            if rest.is_empty() {
                return Err(ApiError::bad_request("a path needs somewhere to go"));
            }

            let seed = seed.unwrap_or(0);
            approach(desktop, spec, *first, *motion, Some(seed)).await?;

            // One list, so the press and the release bracket every leg: a
            // drag_along for each would lift the button at every corner.
            let mut steps = Vec::new();
            let mut at = *first;
            for (leg, to) in rest.iter().enumerate() {
                steps.extend(path(at, *to, *motion, seed.wrapping_add(leg as u64 + 1)));
                at = *to;
            }

            desktop.drag_along(*first, &steps, *button, held).await?
        }
        Action::Type { text, delay_ms } => {
            let pace = delay_ms.map(|ms| Duration::from_millis(ms).min(MAX_PACE));
            desktop.type_text(text, pace).await?
        }
        Action::Press { chord, then, held } => {
            let mut all = vec![chord.clone()];
            all.extend(then.iter().cloned());
            desktop.press(&all, held).await?
        }
        Action::Scroll { at, dx, dy } => desktop.scroll(*at, Delta { dx: *dx, dy: *dy }).await?,
        Action::OpenUrl { url, target } => {
            *doing.page = None;

            match (browser, target) {
                (Some(browser), OpenIn::Blank) => {
                    let opened = browser.open(url).await?;
                    let mut fresh = browser.attach(&opened).await?;
                    // `PUT /json/new` does not raise it.
                    fresh.bring_to_front().await?;

                    doing.tabs.push(tab_out(&opened, true));

                    let _ = browser.tidy(TABS).await;
                }
                (Some(browser), OpenIn::Current) => {
                    let mut showing = match browser.visible_page().await? {
                        Some(showing) => showing,
                        None => browser.first_page().await?,
                    };
                    showing.navigate(url).await?;

                    doing.tabs.push(tab_out(showing.target(), true));
                }
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
            if doing.page.is_none() {
                let browser = browser
                    .ok_or_else(|| ApiError::bad_request("this box publishes no DevTools port"))?;

                *doing.page = browser.visible_page().await?;
            }

            let page = doing
                .page
                .as_mut()
                .ok_or_else(|| ApiError::not_found("no page is on screen"))?;

            apply(page, what.clone(), Duration::ZERO).await?;
        }
        Action::OnNode { what } => {
            on_tree(desktop, what.clone()).await?;
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

            return Ok(Did {
                window: Some(window),
                out: None,
            });
        }
        other => return reaching(doing, other).await,
    }

    Ok(Did::default())
}

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
    #[serde(default)]
    pointer: bool,
    #[serde(default)]
    tab: Option<String>,
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
            pointer: self.pointer,
            tab: self.tab.clone(),
        })
    }
}

async fn frame(
    State(state): State<Arc<AppState>>,
    ApiPath((id, screen)): ApiPath<(String, u32)>,
    ApiQuery(query): ApiQuery<FrameQuery>,
) -> ApiResult<Json<Frame>> {
    let shot = query.shot()?;

    if let Some(tab) = &shot.tab {
        named(&state, &id, tab).await?.bring_to_front().await?;
        tokio::time::sleep(RAISE).await;
    }

    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;

    let png = match shot.is_whole() {
        true => target.as_desktop().screenshot().await?,
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
        pointer: shot.pointer,
    }
}

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

async fn on_node(
    State(state): State<Arc<AppState>>,
    ApiPath((id, screen)): ApiPath<(String, u32)>,
    ApiJson(body): ApiJson<OnNode>,
) -> ApiResult<Json<NodeResult>> {
    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;

    Ok(Json(on_tree(target.as_desktop(), body).await?))
}

/// Where a path starts: the pointer, or the middle of the screen where a driver
/// cannot say where the pointer is.
async fn pointer_start(desktop: &dyn EngineDesktop, spec: &Spec) -> computer_types::Point {
    match desktop.cursor().await {
        Ok(at) => at,
        Err(_) => computer_types::Point {
            x: spec.desktop.width.unwrap_or(1280) / 2,
            y: spec.desktop.height.unwrap_or(800) / 2,
        },
    }
}

async fn approach(
    desktop: &dyn EngineDesktop,
    spec: &Spec,
    to: computer_types::Point,
    motion: computer::Motion,
    seed: Option<u64>,
) -> ApiResult<()> {
    if motion.is_instant() {
        return Ok(());
    }
    let from = pointer_start(desktop, spec).await;
    desktop
        .move_along(&path(from, to, motion, seed.unwrap_or(0)))
        .await?;
    Ok(())
}

async fn on_tree(desktop: &dyn computer::Desktop, what: OnNode) -> ApiResult<NodeResult> {
    Ok(match what {
        OnNode::Tree { app, depth } => NodeResult {
            nodes: desktop.nodes(app.as_deref(), depth).await?,
            ..NodeResult::default()
        },
        OnNode::Find { node, limit } => NodeResult {
            nodes: desktop
                .find_nodes(&node, limit.map(|limit| limit.clamp(1, FOUND)))
                .await?,
            ..NodeResult::default()
        },
        OnNode::Focus { node } => NodeResult {
            node: Some(desktop.focus_node(&node).await?),
            ..NodeResult::default()
        },
        OnNode::Invoke { node, action } => {
            let invoked = desktop.invoke_node(&node, action.as_deref()).await?;
            NodeResult {
                action: invoked.actions.first().cloned(),
                node: Some(invoked),
                ..NodeResult::default()
            }
        }
        OnNode::Set { node, value } => NodeResult {
            node: Some(desktop.set_node(&node, &value).await?),
            ..NodeResult::default()
        },
    })
}

async fn cursor(
    State(state): State<Arc<AppState>>,
    ApiPath((id, screen)): ApiPath<(String, u32)>,
) -> ApiResult<Json<Point>> {
    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;

    Ok(Json(target.as_desktop().find_cursor().await?))
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

/// Through `reclaim`: the `Takeover` handle belonged to a request that has returned.
async fn end_takeover(
    State(state): State<Arc<AppState>>,
    ApiPath((id, screen)): ApiPath<(String, u32)>,
) -> ApiResult<StatusCode> {
    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;
    let held = target
        .as_screen()
        .ok_or_else(|| ApiError::internal("this screen cannot be reclaimed"))?;

    // Captured before release, so the frame is what the person left.
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
    // The gate is per process: a takeover another server started shows only in the box.
    let taken_over = matches!(held.control().control(), computer::Control::Human { .. })
        || held.person_driving().await;

    Ok(Json(ViewersView {
        watching: counts.watching,
        driving: counts.driving,
        person_driving: counts.person_present(),
        taken_over,
    }))
}

async fn recording(
    State(state): State<Arc<AppState>>,
    ApiPath((id, screen)): ApiPath<(String, u32)>,
) -> ApiResult<Json<RecordingView>> {
    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;
    let held = target
        .as_screen()
        .ok_or_else(|| ApiError::internal("this screen cannot be recorded"))?;

    let path = held.recording().await?;

    Ok(Json(RecordingView {
        recording: path.is_some(),
        path,
    }))
}

async fn start_recording(
    State(state): State<Arc<AppState>>,
    ApiPath((id, screen)): ApiPath<(String, u32)>,
    ApiJson(body): ApiJson<StartRecording>,
) -> ApiResult<Json<RecordingView>> {
    if let Some(fps) = body.fps
        && !(1..=60).contains(&fps)
    {
        return Err(ApiError::bad_request("fps must be between 1 and 60"));
    }

    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;
    let held = target
        .as_screen()
        .ok_or_else(|| ApiError::internal("this screen cannot be recorded"))?;

    let path = held.start_recording(body.fps).await?;

    Ok(Json(RecordingView {
        recording: true,
        path: Some(path),
    }))
}

async fn stop_recording(
    State(state): State<Arc<AppState>>,
    ApiPath((id, screen)): ApiPath<(String, u32)>,
) -> ApiResult<Json<RecordingView>> {
    let entry = state.registry.get(&id).await?;
    let target = entry.desktop(screen).await?;
    let held = target
        .as_screen()
        .ok_or_else(|| ApiError::internal("this screen cannot be recorded"))?;

    let path = held.stop_recording().await?;

    Ok(Json(RecordingView {
        recording: false,
        path: Some(path),
    }))
}

async fn pause_box(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<Json<BoxView>> {
    let entry = state.registry.get(&id).await?;
    entry.computer.pause().await?;

    state.record(&id, Actor::Agent, TraceEvent::BoxPaused).await;

    Ok(Json(viewed(&entry, BoxState::Paused)))
}

async fn resume_box(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<Json<BoxView>> {
    let entry = state.registry.get(&id).await?;

    if entry.computer.stopped().await.unwrap_or(false) {
        let woken = entry.computer.start(WAKE).await?;

        let entry = state.registry.replace(&id, woken).await?;
        state
            .record(&id, Actor::Agent, TraceEvent::BoxStarted)
            .await;

        return Ok(Json(viewed(&entry, BoxState::Ready)));
    }

    entry.computer.resume().await?;
    state
        .record(&id, Actor::Agent, TraceEvent::BoxResumed)
        .await;

    Ok(Json(viewed(&entry, BoxState::Ready)))
}

async fn stop_box(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<Json<BoxView>> {
    let entry = state.registry.get(&id).await?;
    entry.computer.stop().await?;

    state
        .record(&id, Actor::Agent, TraceEvent::BoxStopped)
        .await;

    Ok(Json(viewed(&entry, BoxState::Stopped)))
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

async fn list_dir(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(query): ApiQuery<PathQuery>,
) -> ApiResult<Json<Listing>> {
    let entry = state.registry.get(&id).await?;
    let entries = entry.computer.list_dir(&query.path).await?;

    Ok(Json(Listing {
        path: query.path,
        entries,
    }))
}

async fn grep(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<String>,
    ApiJson(body): ApiJson<Search>,
) -> ApiResult<Json<Found>> {
    if body.pattern.is_empty() {
        return Err(ApiError::bad_request(
            "a search with no pattern matches every line",
        ));
    }

    let asked = body
        .limit
        .unwrap_or(computer::MATCHES)
        .clamp(1, computer::MATCHES);
    let entry = state.registry.get(&id).await?;
    let mut matches = entry.computer.grep(&body).await?;

    let cut = matches.len() > asked;
    matches.truncate(asked);

    Ok(Json(Found { matches, cut }))
}

async fn glob(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(query): ApiQuery<GlobQuery>,
) -> ApiResult<Json<Globbed>> {
    let asked = query
        .limit
        .unwrap_or(computer::MATCHES)
        .clamp(1, computer::MATCHES);
    let entry = state.registry.get(&id).await?;
    let mut paths = entry
        .computer
        .glob(
            &query.pattern,
            query.path.as_deref().unwrap_or("/"),
            query.limit,
        )
        .await?;

    let cut = paths.len() > asked;
    paths.truncate(asked);

    Ok(Json(Globbed { paths, cut }))
}

#[derive(Debug, Deserialize)]
struct GlobQuery {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
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
    // Held across the replay: `desktop` reruns the screen start command on each call.
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
            // A refused action did not happen, so replaying it would invent a difference.
            TraceEvent::Acted { .. } => continue,
            TraceEvent::Executed { argv, .. } if !argv.is_empty() => {
                Step::Exec { argv: argv.clone() }
            }
            TraceEvent::AppLaunched {
                screen, app, args, ..
            } => Step::Act {
                screen: *screen,
                action: Action::Launch {
                    app: app.clone(),
                    args: args.clone(),
                },
            },
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
                            &mut Doing {
                                state,
                                id,
                                number: *screen,
                                entry,
                                desktop: target.as_desktop(),
                                screen: target.as_screen(),
                                spec: &entry.spec,
                                browser: None,
                                page: &mut None,
                                tabs: &mut Vec::new(),
                            },
                            action,
                        )
                        .await
                    }
                    Err(error) => Err(error),
                };

                match (&acted, action) {
                    (
                        Ok(Did {
                            window: Some(window),
                            ..
                        }),
                        Action::Launch { app, args },
                    ) => {
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

async fn read_page(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(query): ApiQuery<PageQuery>,
) -> ApiResult<Json<PageText>> {
    let mut page = page_for(&state, &id, query.tab.as_deref()).await?;

    Ok(Json(
        read_out(
            &mut page,
            &PageRead {
                format: query.format.unwrap_or_default(),
                limit: query.limit,
                max_links: query.max_links,
                tab: query.tab,
            },
        )
        .await?,
    ))
}

async fn read_out(page: &mut computer::Page, what: &PageRead) -> ApiResult<PageText> {
    let format = match what.format {
        Reading::Markdown => computer::Reading::Markdown,
        Reading::Text => computer::Reading::Text,
        Reading::Raw => computer::Reading::Raw,
    };

    let read = page
        .read(
            format,
            Some(what.limit.unwrap_or(PAGE_TEXT).clamp(1, PAGE_TEXT)),
            Some(what.max_links.unwrap_or(LINKS).clamp(0, LINKS)),
        )
        .await?;

    Ok(PageText {
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
    })
}

#[derive(Debug, Deserialize)]
struct FindQuery {
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    scroll: Option<bool>,
    #[serde(default)]
    exact: Option<bool>,
    #[serde(default)]
    tab: Option<String>,
    #[serde(default)]
    role: Option<String>,
}

async fn find_elements(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(query): ApiQuery<FindQuery>,
) -> ApiResult<Json<Vec<Element>>> {
    let mut page = page_for(&state, &id, query.tab.as_deref()).await?;

    let what = match query.role.as_deref() {
        Some(role) => computer::cdp::selector_for(role)
            .ok_or_else(|| {
                ApiError::bad_request(format!(
                    "no such role: {role}. This server knows {}",
                    computer::cdp::ROLES
                ))
            })?
            .to_string(),
        None => query
            .q
            .clone()
            .ok_or_else(|| ApiError::bad_request("a find needs a query or a role"))?,
    };

    let found = page
        .find(
            &what,
            Some(query.limit.unwrap_or(FOUND).clamp(1, FOUND)),
            query.scroll,
            query.exact,
        )
        .await?;

    Ok(Json(found.into_iter().map(element_out).collect()))
}

#[derive(Debug, Deserialize)]
struct SnapshotQuery {
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    tab: Option<String>,
    #[serde(default)]
    delta: Option<bool>,
    #[serde(default)]
    quiet_ms: Option<u64>,
}

async fn snapshot_page(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(query): ApiQuery<SnapshotQuery>,
) -> ApiResult<Json<Snapshot>> {
    let mut page = page_for(&state, &id, query.tab.as_deref()).await?;

    if let Some(quiet) = query.quiet_ms {
        let quiet = Duration::from_millis(quiet).min(MAX_WAIT);
        page.quiet(quiet, Duration::from_millis(WAIT_MS).max(quiet))
            .await?;
    }

    let limit = Some(query.limit.unwrap_or(SNAPSHOT).clamp(1, SNAPSHOT));
    let taken = match query.delta.unwrap_or(false) {
        true => page.snapshot_delta(query.scope.as_deref(), limit).await?,
        false => page.snapshot(query.scope.as_deref(), limit).await?,
    };

    Ok(Json(snapshot_out(taken)))
}

async fn on_element(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(query): ApiQuery<SettleQuery>,
    ApiJson(body): ApiJson<OnElement>,
) -> ApiResult<Json<ElementResult>> {
    // Checked first: the debugger uploads a missing path as an empty file and succeeds.
    if let OnElement::Upload { paths, .. } = &body {
        missing(&state, &id, paths).await?;
    }

    let mut page = page_for(&state, &id, query.tab.as_deref()).await?;
    let settle = Duration::from_millis(query.settle_ms.unwrap_or_default()).min(MAX_PAUSE);

    Ok(Json(apply(&mut page, body, settle).await?))
}

async fn missing(state: &AppState, id: &str, paths: &[String]) -> ApiResult<()> {
    if paths.is_empty() {
        return Err(ApiError::bad_request(
            "an upload with no files hands over nothing",
        ));
    }

    let entry = state.registry.get(id).await?;

    for path in paths {
        let mut argv = vec!["test".to_string(), "-f".to_string()];
        argv.push(path.clone());

        if !entry.computer.exec(&argv).await?.ok() {
            return Err(ApiError::bad_request(format!(
                "the box has no file at {path}. Write it there first."
            )));
        }
    }

    Ok(())
}

#[derive(Debug, Deserialize)]
struct SettleQuery {
    #[serde(default)]
    settle_ms: Option<u64>,
    #[serde(default)]
    tab: Option<String>,
}

async fn apply(
    page: &mut computer::Page,
    what: OnElement,
    settle: Duration,
) -> ApiResult<ElementResult> {
    // A value set on an input is not a mutation, so only a press is watched:
    // a fill would read as nothing having changed.
    let watched = matches!(
        what,
        OnElement::Click { .. } | OnElement::Hover { .. } | OnElement::Drag { .. }
    );

    // A courtesy: a page that refuses to be listed still gets its click.
    let listed = !matches!(what, OnElement::Options { .. });
    if listed {
        let _ = page.remember().await;
    }

    let before = page.url().await.ok();
    let watching = watched && page.watch().await.is_ok();
    let mut result = applied(page, what).await?;

    if !settle.is_zero() {
        tokio::time::sleep(settle).await;
    }

    if let Ok(after) = page.url().await {
        result.navigated = before.as_deref() != Some(after.as_str());
        result.url = Some(after);
    }

    if watching {
        result.changed = page.changed().await.ok().flatten();
    }

    // A new document has no numbers and reports nothing; a pushState keeps both.
    if listed {
        result.delta = page
            .changes(Some(DELTA_LINES))
            .await
            .ok()
            .flatten()
            .filter(|changes| {
                !(changes.added.is_empty() && changes.changed.is_empty() && changes.gone.is_empty())
            })
            .map(changes_out);
    }

    Ok(result)
}

async fn applied(page: &mut computer::Page, what: OnElement) -> ApiResult<ElementResult> {
    Ok(match what {
        OnElement::Click {
            query,
            button,
            double,
            motion,
            seed,
        } => {
            let seed = seed.unwrap_or(0);
            let on = match double {
                true => {
                    page.double_click_on_with(&query, button, motion, seed)
                        .await?
                }
                false => page.click_on_with(&query, button, motion, seed).await?,
            };

            ElementResult {
                element: Some(element_out(on)),
                ..ElementResult::default()
            }
        }
        OnElement::Fill { query, text } => {
            page.fill(&query, &text).await?;
            ElementResult::default()
        }
        OnElement::Options { query } => ElementResult {
            options: page.options(&query).await?,
            ..ElementResult::default()
        },
        OnElement::Focus { query } => ElementResult {
            element: Some(element_out(page.focus(&query).await?)),
            ..ElementResult::default()
        },
        OnElement::Check { query, on } => ElementResult {
            element: Some(element_out(page.check(&query, on).await?)),
            ..ElementResult::default()
        },
        OnElement::Choose {
            query,
            options,
            drop,
        } => ElementResult {
            options: page.choose(&query, &options, drop).await?,
            ..ElementResult::default()
        },
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
            quiet_ms,
            enabled,
            load,
            until,
        } => {
            if query.is_empty() && quiet_ms.is_none() && !load && until.is_none() {
                return Err(ApiError::bad_request(
                    "a wait needs a query, quiet_ms, load or until",
                ));
            }

            let within = Duration::from_millis(within_ms.unwrap_or(WAIT_MS)).min(MAX_WAIT);
            let started = Instant::now();
            let mut result = ElementResult::default();

            // First: a query run against a document still loading is asked of
            // a page that is not there yet.
            if load {
                page.wait_for_load(within).await?;
            }

            if !query.is_empty() {
                let (matched, found) = page
                    .wait_until(
                        &query,
                        &or,
                        gone,
                        within,
                        exact,
                        computer::cdp::Ready { enabled },
                    )
                    .await?;
                result.element = found.map(element_out);
                result.matched = Some(matched);
            }

            if let Some(until) = &until {
                let left = within.saturating_sub(started.elapsed());
                page.wait_until_true(until, left).await?;
            }

            // One window for both: the quiet is what the query waited for, landing.
            if let Some(quiet) = quiet_ms {
                let left = within.saturating_sub(started.elapsed());
                page.quiet(Duration::from_millis(quiet).min(MAX_WAIT), left)
                    .await
                    .map_err(|error| match error {
                        computer::Error::Timeout { detail, .. } => computer::Error::Timeout {
                            after: within,
                            detail,
                        },
                        other => other,
                    })?;
            }

            result
        }
        OnElement::Hover {
            query,
            motion,
            seed,
        } => ElementResult {
            element: Some(element_out(
                page.hover_with(&query, motion, seed.unwrap_or(0)).await?,
            )),
            ..ElementResult::default()
        },
        OnElement::Drag {
            from,
            to,
            button,
            motion,
            seed,
        } => {
            let (source, _) = page
                .drag_on(&from, &to, button, motion, seed.unwrap_or(0))
                .await?;
            ElementResult {
                element: Some(element_out(source)),
                ..ElementResult::default()
            }
        }
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
                at: Some(Point {
                    x: x.max(0) as u32,
                    y: y.max(0) as u32,
                }),
                ..ElementResult::default()
            }
        }
    })
}

async fn evaluate(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(query): ApiQuery<TabQuery>,
    ApiJson(body): ApiJson<Evaluate>,
) -> ApiResult<Json<Evaluated>> {
    let mut page = page_for(&state, &id, query.tab.as_deref()).await?;

    let within = Duration::from_millis(body.timeout_ms.unwrap_or(EVALUATE_MS)).min(MAX_PAUSE);
    let value = page.evaluate_within(&body.expression, Some(within)).await?;

    let json = serde_json::to_string(&value)
        .map_err(|error| ApiError::internal(format!("the answer would not serialise: {error}")))?;

    let limit = body.limit.unwrap_or(EVALUATED).clamp(1, EVALUATED);
    let truncated = json.chars().count() > limit;

    Ok(Json(Evaluated {
        json: json.chars().take(limit).collect(),
        truncated,
    }))
}

async fn page_screenshot(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<String>,
    ApiJson(body): ApiJson<PageShot>,
) -> ApiResult<Json<Captured>> {
    Ok(Json(captured_page(&state, &id, &body).await?))
}

async fn captured_page(state: &AppState, id: &str, body: &PageShot) -> ApiResult<Captured> {
    if let Some(quality) = body.quality
        && !(1..=100).contains(&quality)
    {
        return Err(ApiError::bad_request("quality is between 1 and 100"));
    }

    // JPEG by default for a full page: 6.8MB as PNG against 115KB as JPEG.
    let format = body.format.unwrap_or(match body.full {
        true => Picture::Jpeg,
        false => Picture::Png,
    });

    let shot = computer::cdp::PageShot {
        full: body.full,
        format: match format {
            Picture::Png => computer::cdp::Picture::Png,
            Picture::Jpeg => computer::cdp::Picture::Jpeg,
        },
        quality: body.quality.unwrap_or(computer::cdp::JPEG_QUALITY),
    };

    let mut page = page_for(state, id, body.tab.as_deref()).await?;
    let image = page.capture(&shot).await?;

    state
        .record(
            id,
            Actor::Agent,
            TraceEvent::PageCaptured {
                full: body.full,
                bytes: image.len(),
            },
        )
        .await;

    Ok(Captured {
        format,
        bytes: image.len(),
        image_base64: BASE64.encode(&image),
    })
}

#[derive(Debug, Deserialize)]
struct TabQuery {
    #[serde(default)]
    tab: Option<String>,
}

async fn list_tabs(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<Json<Vec<Tab>>> {
    Ok(Json(listed_tabs(&state, &id).await?))
}

async fn listed_tabs(state: &AppState, id: &str) -> ApiResult<Vec<Tab>> {
    let browser = debugger(state, id).await?;

    let showing = match browser.visible_page().await {
        Ok(Some(page)) => Some(page.target().id.clone()),
        _ => None,
    };

    Ok(browser
        .pages()
        .await?
        .iter()
        .map(|target| tab_out(target, showing.as_deref() == Some(target.id.as_str())))
        .collect())
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

async fn debugger(state: &AppState, id: &str) -> ApiResult<computer::Devtools> {
    let entry = state.registry.get(id).await?;

    entry
        .computer
        .browser()
        .ok_or_else(|| ApiError::bad_request("this box publishes no DevTools port"))
}

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

fn snapshot_out(taken: computer::Snapshot) -> Snapshot {
    Snapshot {
        url: taken.url,
        title: taken.title,
        total: taken.total,
        elements: taken.elements.into_iter().map(element_out).collect(),
        delta: taken.delta.map(changes_out),
    }
}

fn changes_out(delta: computer::Changes) -> Changes {
    Changes {
        first: delta.first,
        added: delta.added.into_iter().map(element_out).collect(),
        changed: delta.changed.into_iter().map(element_out).collect(),
        gone: delta.gone.into_iter().map(element_out).collect(),
        same: delta.same,
    }
}

fn element_out(element: computer::Element) -> Element {
    Element {
        text: element.text,
        tag: element.tag,
        kind: element.kind,
        role: element.role,
        states: element.states,
        selector: element.selector,
        label: element.label,
        visible: element.visible,
        at: element.at.map(|at| Point { x: at.x, y: at.y }),
        width: element.width,
        height: element.height,
        enabled: element.enabled,
        value: element.value,
        r#ref: element.r#ref,
        href: element.href,
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

/// Untraced: on a fork whose windows opened in another order it would raise the wrong one.
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

async fn catalog() -> Json<BTreeMap<String, computer_types::App>> {
    Json(computer::apps::builtin())
}

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
    viewed(entry, BoxState::Ready)
}

fn viewed(entry: &Entry, state: BoxState) -> BoxView {
    // A stopped box's held ports may already belong to another box.
    let reachable = state != BoxState::Stopped;

    BoxView {
        id: entry.id.clone(),
        spec_digest: entry.spec_digest(),
        state,
        screens: entry.screens,
        width: entry.width,
        height: entry.height,
        viewer_url: entry.computer.viewer_url().filter(|_| reachable),
        devtools_url: entry
            .computer
            .devtools()
            .map(|endpoint| endpoint.http_url.clone())
            .filter(|_| reachable),
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
    // An id grants control, so it comes from the CSPRNG, not the clock.
    if getrandom::fill(&mut bytes).is_err() {
        return format!("box_{}", millis(SystemTime::now()));
    }

    let mut id = String::from("box_");
    for byte in bytes {
        id.push_str(&format!("{byte:02x}"));
    }
    id
}
