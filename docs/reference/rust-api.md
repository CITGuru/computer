# Rust API reference

The `computer` crate drives a box from Rust with no server. This page gives the main types and methods. For every item, build the API docs:

```bash
cargo doc -p computer --no-deps --open
```

To add the dependency, see the [Rust quick start](../getting-started/quickstart-rust.md).

## Main types

| Type | Purpose |
| --- | --- |
| `Computer` | One box. Owns the box and its screens. |
| `Builder` | Configures a box before launch. From `Computer::builder()`. |
| `Screen` | One screen of a box. `computer.primary()` is screen 0. |
| `Devtools` | The box's browser, through Chrome DevTools. From `computer.browser()`. |
| `Page` | One browser tab. |
| `Takeover` | A takeover by a person. From `hand_over()` or `share()`. |
| `Error`, `Result` | The error type for all operations. |

`Computer` has the same input, screenshot, clipboard, widget, and takeover methods as `Screen`. They act on the primary screen.

## Create and remove a box

| Method | Effect |
| --- | --- |
| `Computer::launch()` | Launch a box with the defaults: one 1280x800 screen on Docker. |
| `Computer::builder()…launch()` | Launch a box with options. See [Builder](#builder). |
| `Computer::attach(name)` | Attach to a running box by name. |
| `computer.shutdown()` | Stop and remove the box. |
| `computer.pause()`, `resume()` | Pause and resume. |
| `computer.stop()`, `start(within)` | Stop the box and keep its files, then start a new desktop. `start` returns a new `Computer`. |

When a `Computer` is dropped, its box is removed, unless the builder set `keep_on_drop(true)`.

## Builder

```rust
let computer = Computer::builder()
    .size(1920, 1080)
    .accessibility()
    .network(false)
    .memory("2g")
    .expires_after(Duration::from_secs(3600))
    .launch()
    .await?;
```

| Method | Effect |
| --- | --- |
| `size(w, h)` | Screen size. |
| `packages([…])` | Apt packages to install. |
| `wide_fonts()`, `video()`, `dock()`, `accessibility()` | Features. See [Boxes](../concepts/boxes.md#what-is-inside-a-box). |
| `profile(Arc::new(WaylandProfile))` | Use Wayland instead of X11. |
| `network(false)` | Block network access. |
| `memory(limit)`, `cpus(n)` | Resource limits. |
| `runtime(program)` | `docker` (default), `podman`, or `nerdctl`. |
| `machine(Arc<dyn Machine>)` | Another runtime, such as a microVM, E2B, Vercel, Daytona, or Modal. |
| `name(name)` | The box name, for `attach`. |
| `image(tag)` | Use an image that is already built. |
| `profiles(volume)` | Keep the browser profile in a named volume. |
| `expires_after(duration)` | Remove the box after a fixed time. |
| `expires_when_idle(duration)` | Remove the box after a time with no use. Call `touch()` when work reaches the box by another path. |
| `keep_on_drop(true)` | Keep the box when the `Computer` is dropped. |
| `auth(Auth)`, `publish_on(Bind)`, `advertise(host)` | Viewer access. See [Human control](../concepts/human-control.md#viewer-access). |
| `config()` | Return the resolved configuration and do not launch. |
| `Builder::from_spec(&spec)` | Start a builder from a box spec. Use it for settings with no builder method, such as the number of screens and catalog applications. |

## Screen input

Coordinates are device pixels. `(0, 0)` is the top-left corner. A point is `Point::new(x, y)` or `(x, y)`.

| Method | Effect |
| --- | --- |
| `screenshot()` | Return a PNG of the screen. |
| `capture(&Shot)` | Capture a window or a rectangle. |
| `move_to(at)` | Move the pointer. |
| `click(at, button)`, `double_click(at, button)` | Click. |
| `click_with(…)`, `drag_with(…)` | Click or drag with modifier keys held, or with motion. |
| `drag(from, to, button)` | Drag. |
| `scroll(at, Delta::down(3))` | Turn the wheel, in notches. |
| `type_text(text)` | Type into the focused element. Add `.every(duration)` to slow it down. |
| `press("ctrl+l")` | Press a key or combination. |
| `cursor()` | Get the pointer position. |
| `wait_until_still(settle, within)` | Wait until the screen stops changing. |

## Windows and applications

On `Screen` (`computer.primary()`):

| Method | Effect |
| --- | --- |
| `windows()` | List windows. |
| `active_window()` | Get the window that receives keyboard input. |
| `wait_for_window(class, within)` | Wait for a window to appear. |
| `focus(window)`, `close_window(window)` | Bring to the front, or close. |
| `arrange(window, Arrange)` | Move, resize, maximize, minimize, or restore. |
| `launch(&Launch)` | Open an application. |

## Browser

```rust
let browser = computer.browser().ok_or("no DevTools port")?;
let mut page = browser
    .open_page("https://example.com", Duration::from_secs(30))
    .await?;

page.fill("Email", "me@example.com").await?;
page.click_on("Sign in", Button::Left).await?;
page.wait_for("Dashboard", false, Duration::from_secs(10)).await?;
let text = page.read(Reading::Markdown, None, None).await?;
```

`browser()` returns `None` when the box has no DevTools port. On E2B, Vercel, Daytona, and Modal, see [Browser mode on remote runtimes](../concepts/control-modes.md#browser-mode-on-remote-runtimes).

`Devtools`:

| Method | Effect |
| --- | --- |
| `open_page(url, within)` | Open a tab and return its `Page`. |
| `pages()` | List tabs. |
| `visible_page()` | The tab that the screen shows. |
| `cookies(url)`, `set_cookies(…)`, `clear_cookies(url)` | Cookies. |
| `export_session(origins, carry)`, `import_session(&session)` | Move a login between boxes. |
| `create_group()` | An isolated browser session. |

`Page`:

| Method | Effect |
| --- | --- |
| `read(format, limit, max_links)` | The page as Markdown, text, or HTML. |
| `snapshot(scope, limit)`, `snapshot_delta(…)` | List the controls, each with a reference such as `@e12`. |
| `find(query, limit, scroll, exact)` | Find elements. |
| `click_on(query, button)`, `double_click_on(…)`, `hover(query)`, `drag_on(…)` | Pointer actions on an element. |
| `fill(query, text)`, `focus(query)`, `check(query, on)` | Field actions. |
| `options(query)`, `choose(query, options, drop)` | Dropdowns. |
| `upload(query, paths)` | File inputs. Paths are in the box. |
| `wait_for(query, gone, within)`, `wait_for_any(…)`, `wait_until_true(js, within)`, `wait_for_load(within)` | Waits. |
| `evaluate(js)` | Run JavaScript. |
| `navigate(url)`, `back()`, `forward()`, `reload()` | Navigation. |
| `bring_to_front()`, `visible()` | Make this tab the one the screen shows. |
| `screenshot()`, `capture(&PageShot)`, `pdf(landscape, background)` | Captures. |
| `console(clear)` | The console log. |
| `call(method, params)` | Send any CDP command. |

## Native widgets

Needs `accessibility()` on the builder. See [Control modes](../concepts/control-modes.md#accessibility).

| Method | Effect |
| --- | --- |
| `nodes(app, depth)` | The widget tree. |
| `find_nodes(&NodeQuery, limit)` | Find widgets. |
| `focus_node(&NodeQuery)` | Give a widget the keyboard focus. |
| `set_node(&NodeQuery, value)` | Set a field. |
| `invoke_node(&NodeQuery, action)` | Run a widget action. |

## Files, commands, and clipboard

| Method | Effect |
| --- | --- |
| `exec(argv)`, `exec_within(argv, within)` | Run a command. Returns the exit code, stdout, and stderr. |
| `read_file(path)`, `write_file(path, bytes)` | Read or write a file in the box. |
| `upload(from, to)`, `download(from, to)` | Copy a file between the host and the box. |
| `list_dir(path)`, `grep(&Search)`, `glob(…)` | Search the box. |
| `clipboard()`, `set_clipboard(text)` | The clipboard. |
| `selection(Selection::Primary)`, `set_selection(…)` | The primary selection. |

## Human control

See [Human control](../concepts/human-control.md).

| Method | Effect |
| --- | --- |
| `viewer_url()` | The watch URL. |
| `hand_over()` | Start an exclusive takeover. The `Takeover` has the control URL. |
| `share()` | Start a shared takeover. |
| `wait_until_free(within)` | Wait until no person has the control viewer open. |
| `takeover.end()` | End the takeover. |
| `person_driving()`, `reclaim()` | Check for and end a takeover that has no owner. |
| `start_recording(fps)`, `stop_recording()` | Record the screen. Needs `video()`. |

## Errors

All methods return `computer::Result<T>`. The variants of `computer::Error`:

| Variant | Meaning |
| --- | --- |
| `Unavailable` | The runtime or a program is not available. |
| `Unsupported` | The box cannot do this. `gaps` names what is missing. |
| `Invalid` | The request is not valid. |
| `Gone` | The box is gone. |
| `Denied` | Refused. Usually a person has control of the screen. |
| `Failed` | A command ran and did not succeed. |
| `Timeout` | An operation did not finish in time. |
| `ScreenUnavailable` | The screen cannot be used now. |
| `Transport` | The box could not be reached. |

`error.retryable()` tells you if the same call can succeed later.

## Extension points

| Trait | Implement it to |
| --- | --- |
| `Machine` | Run boxes on a different platform. |
| `Profile` | Use a different desktop image or display stack. |
| `Engine` | Use a different container CLI. |
| `MicroVmApi` | Add a hypervisor. |
| `RemoteApi` | Add a cloud sandbox vendor. |

See `examples/custom_image.rs` and `examples/custom_sandbox.rs`.

## Examples

Run each with `cargo run --example <name>`.

| Example | Shows |
| --- | --- |
| `quickstart` | Launch, open a page, click, type, screenshot |
| `serve` | Launch a box that stays after the program exits, and print its viewer and DevTools URLs |
| `tour` | Open a box, drive it, and keep it |
| `attach` | Attach to a running box by name and type into it |
| `browser` | Page operations through DevTools |
| `elements` | Find, fill, dropdowns, and file inputs on an attached box |
| `waiting` | Page waits instead of sleeps |
| `capture` | Screenshots of the screen, a rectangle, and a window |
| `recording` | A GIF made from frames |
| `takeover` | Handing control to a person |
| `from_spec` | Launching from a box spec (`box.json`) |
| `custom_image` | A custom image |
| `custom_sandbox` | A custom remote vendor |
| `microvm` | A microVM through microsandbox |
| `e2b`, `e2b_takeover` | E2B, with and without a takeover. Need `--features e2b` and `E2B_API_KEY`. |
| `vercel` | Vercel Sandbox. Needs `--features vercel` and the `VERCEL_*` variables. |
| `daytona` | Daytona. Needs `--features daytona` and `DAYTONA_API_KEY`. |
| `modal` | Modal. Needs `--features modal`, `MODAL_TOKEN_ID`, and `MODAL_TOKEN_SECRET`. |
