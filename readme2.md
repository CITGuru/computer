# Computer

`computer` provides isolated desktop boxes that agents can use to complete and drive real tasks. Each computer runs a Linux desktop inside a container or microVM, where an agent can open web pages, drive native apps, take screenshots, move the pointer, type text, run commands, and transfer files. You can also watch the agent work or take control through a browser when it needs help.

Nine frames of a desktop being driven from Rust: a page opening, a URL typed, text selected by a drag, a context menu, a paste, and a second screen

## Contents

- [Quick start](#quick-start)
- [Choose how to control the desktop](#choose-how-to-control-the-desktop)
- [Desktop operations](#desktop-operations)
- [Accessibility operations](#accessibility-operations)
- [Browser operations](#browser-operations)
- [Display and window operations](#display-and-window-operations)
- [Human control](#human-control)
- [Configure a desktop](#configure-a-desktop)
- [Runtimes](#runtimes)
- [Custom desktops and runtimes](#custom-desktops-and-runtimes)
- [Security](#security)
- [Examples](#examples)
- [Development and testing](#development-and-testing)



## Quick start



### Requirements

Install Rust and one supported runtime:

- Docker
- Podman
- nerdctl
- microsandbox, for a microVM instead of a container

You do not need to download or build a desktop image. The crate contains the image source and builds it when you launch the first desktop.

### Install the commands

Install a release build on macOS or Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/CITGuru/computer/main/scripts/install.sh | sh
```

The installer supports x86-64 and arm64. It installs:

- `computer`, which controls desktops from a shell and serves MCP tools
- `computerd`, which keeps the REST service running

Set `COMPUTER_INSTALL_DIR` to change the install directory. Set `COMPUTER_VERSION` to install a specific tag.

Start and control a desktop:

```bash
BOX=$(computer new)
computer open "$BOX" https://example.com
computer screenshot "$BOX" screen.png
computer rm "$BOX"
```

See the [CLI guide](crates/computer-cli/README.md) for server and remote-fleet use.

### Use the Rust API

Add the API without the command dependencies:

```toml
[dependencies]
computer = { git = "https://github.com/CITGuru/computer", default-features = false }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Launch a desktop, open a page, send input, and take a screenshot:

```rust
use computer::{Button, Computer, Point};

#[tokio::main]
async fn main() -> computer::Result<()> {
    let computer = Computer::launch().await?;

    computer.open_url("https://example.com").await?;
    computer.click(Point::new(640, 81), Button::Left).await?;
    computer.type_text("driven from rust").await?;

    let _png = computer.screenshot().await?;

    computer.shutdown().await
}
```

CLI:

```bash
BOX=$(computer new --url https://example.com)
computer mouse "$BOX" click 640 81 left
computer keyboard "$BOX" type "driven from the CLI"
computer screenshot "$BOX" screen.png
computer rm "$BOX"
```

`Computer::launch()` starts one 1280x800 screen. `viewer_url()` returns a browser URL where you can watch it. `shutdown()` stops and removes the container.

From this repository, run the complete example:

```bash
cargo run --example quickstart
```



## Choose how to control the desktop

The crate provides three control methods. You can use more than one method in the same task.

**Screen coordinates** work with any visible application. Take a screenshot, select a point, and send mouse or keyboard input. Use this method when the application exposes no structured interface.

**Accessibility nodes** expose native widgets by role and name. Use them for file dialogs, settings panels, installers, and other native interfaces where coordinates can become stale.

**Chrome DevTools** controls Chromium pages directly. Use it to navigate, inspect the DOM, find elements, fill forms, evaluate JavaScript, and manage isolated browser sessions.

## Desktop operations



### Mouse and keyboard

Coordinates use device pixels. The top-left corner is `(0, 0)`.

```rust
let png = computer.screenshot().await?;

computer.move_to((640, 400)).await?;
computer.click((640, 400), Button::Left).await?;
computer.double_click((640, 400), Button::Left).await?;
computer.drag((100, 100), (400, 300), Button::Left).await?;

computer.type_text("hello").await?;
computer.key("ctrl+shift+p").await?;

computer.scroll((640, 400), Delta::down(3)).await?;
computer.scroll((640, 400), Delta::right(3)).await?;

let pointer = computer.cursor().await?;
```

CLI:

```bash
computer screenshot "$BOX" frame.png
computer mouse "$BOX" move 640 400
computer mouse "$BOX" click 640 400 left
computer mouse "$BOX" click 640 400 left --double
computer mouse "$BOX" drag 100 100 400 300 left
computer keyboard "$BOX" type "hello"
computer keyboard "$BOX" key "ctrl+shift+p"
computer mouse "$BOX" scroll 640 400 down 3
computer mouse "$BOX" scroll 640 400 right 3
computer mouse "$BOX" at
```

`scroll` moves the content below the given point. Positive `dy` moves down and positive `dx` moves right. A delta with both values sends one diagonal gesture.

Common key names work as expected. The crate converts names such as `enter`, `cmd`, and `pageup` to the display server's key names.

Keep these coordinate rules:

- Screenshots do not show the pointer. Use `cursor()` to read its position.
- Do not calculate a click from a scaled screenshot.
- Take a new screenshot after a person or another process changes the desktop.
- Take a new screenshot after an operation opens or raises a browser tab.

Hold modifiers as part of the pointer operation:

```rust
screen.click_with(at, Button::Left, &[Held::Shift]).await?;
screen.drag_with(from, to, Button::Left, &[Held::Ctrl]).await?;
```

CLI:

```bash
computer mouse "$BOX" click 640 400 left --held shift
computer mouse "$BOX" drag 100 100 400 300 left --held ctrl
```

Modifier pointer operations are available on X11 and on Wayland.

### Wait for the screen

Wait for drawing to stop instead of using a fixed sleep:

```rust
screen
    .wait_until_still(
        Duration::from_millis(400),
        Duration::from_secs(10),
    )
    .await?;
```

CLI:

```bash
computer still "$BOX" --settle 400 --within 10000
```

An animated screen can reach the deadline without becoming still.

### Screenshots and captures

`screenshot()` returns a full-size PNG. `capture()` can select a region or window and can reduce its size:

```rust
let screen = computer.primary();

let full = screen.screenshot().await?;
let region = screen
    .capture(&Shot::region(Rect::new(
        Point::new(100, 80),
        400,
        300,
    )))
    .await?;
let window = screen.capture(&Shot::window(&window_id)).await?;
let small = screen
    .capture(&Shot::window(&window_id).scaled(50))
    .await?;
```

CLI:

```bash
computer screenshot "$BOX" full.png
computer screenshot "$BOX" region.png --at 100,80 --size 400x300
computer screenshot "$BOX" window.png --window 42
computer screenshot "$BOX" small.png --window 42 --scale 50
```

A window capture looks up the current window position when it runs. Scaling reduces the bytes sent to an agent, but scaled captures must not be used to calculate input coordinates.

### Files and commands

Run commands and move files between the host and the desktop:

```rust
let output = computer.exec(["ls", "-la", "/tmp"]).await?;
computer.write_file("/tmp/input.png", &bytes).await?;
computer.download("/tmp/output.gif", "output.gif").await?;

let logs = computer.logs().await?;
```

CLI:

```bash
computer exec "$BOX" -- ls -la /tmp
```

Commands have a two-minute default limit. Use `exec_within(argv, duration)` when a command needs a different limit.

### Clipboard

Each screen has its own clipboard and primary selection:

```rust
computer.set_clipboard("ready to paste").await?;
let text = computer.clipboard().await?;

computer
    .set_selection(Selection::Primary, "middle-click paste")
    .await?;
let selected = computer.selection(Selection::Primary).await?;
```

CLI:

```bash
computer clip "$BOX" "ready to paste"
computer clip "$BOX"
computer clip "$BOX" "middle-click paste" --primary
computer clip "$BOX" --primary
```

`CLIPBOARD` is used by copy and paste. `PRIMARY` is filled when text is selected and is pasted by a middle click.

Selections can also contain binary data:

```rust
let targets = computer
    .clipboard_targets(Selection::Clipboard)
    .await?;
let png = computer
    .clipboard_bytes(Selection::Clipboard, "image/png")
    .await?;
computer
    .set_clipboard_bytes(Selection::Clipboard, "image/png", &bytes)
    .await?;
```

While a person controls the screen, the program can read selections but cannot change them.

## Accessibility operations

Accessibility operations use the semantic tree that native applications publish through AT-SPI. Enable the required image packages when you launch the desktop:

```rust
let computer = Computer::builder()
    .accessibility()
    .launch()
    .await?;
let screen = computer.primary();
```

CLI:

```bash
BOX=$(computer new --accessibility)
```



### Read and find nodes

Read the tree or search it by widget name and role:

```rust
let tree = screen.nodes(None, Some(4)).await?;

let street = NodeQuery {
    query: "Street".to_string(),
    role: Some("text".to_string()),
    ..NodeQuery::default()
};

let fields = screen.find_nodes(&street, Some(10)).await?;
```

CLI:

```bash
computer widget "$BOX" tree --depth 4
computer widget "$BOX" find "Street" --role text --limit 10
```

`find_nodes` returns the best match first. A query can match a widget's own name or the label beside it. `Node::labelled` shows when a nearby label produced the match.

### Focus, set, and invoke nodes

```rust
screen.focus_node(&street).await?;
screen.set_node(&street, "12 Bishop Street").await?;

let ok = NodeQuery {
    query: "OK".to_string(),
    ..NodeQuery::default()
};
screen.invoke_node(&ok, None).await?;
```

CLI:

```bash
computer widget "$BOX" focus "Street" --role text
computer widget "$BOX" fill "Street" "12 Bishop Street" --role text
computer widget "$BOX" press "OK"
```

`invoke_node` runs a widget action without moving the pointer. The toolkit controls the action names. GTK can call an action `click` while Qt calls it `Press`.

Each node can also contain its centre point. Use that point when the application must receive a real pointer click:

```rust
if let Some(at) = fields.first().and_then(|node| node.at) {
    screen.click(at, Button::Left).await?;
}
```

CLI:

```bash
computer mouse "$BOX" click 640 400 left
```

Applications and custom widgets can publish incomplete trees. Use screenshots and normal input when a useful node is not available.



## Browser operations



### Open and manage pages

Chrome DevTools controls a page without screen coordinates:

```rust
let browser = computer
    .browser()
    .expect("a published DevTools port");

let mut page = browser
    .open_page("https://example.com", Duration::from_secs(20))
    .await?;

let title = page.title().await?;
let links = page
    .evaluate("Array.from(document.links).map(a => a.href)")
    .await?;
let png = page.screenshot().await?;

page.navigate("https://example.org").await?;
```

CLI:

```bash
TAB=$(computer open "$BOX" https://example.com)
computer browser "$BOX" eval "document.title" --tab "$TAB"
computer browser "$BOX" eval "Array.from(document.links).map(a => a.href)" --tab "$TAB"
computer browser "$BOX" screenshot page.png --tab "$TAB"
computer open "$BOX" https://example.org --target current
```

`Page::call` sends any Chrome DevTools Protocol method that the crate does not wrap.

Use `open_page` rather than `open` followed by a load wait. A new tab first shows `about:blank`, which is already loaded.

### Find and act on elements

Pages provide higher-level operations for common browser tasks:

```rust
let fields = page.find("input", Some(10), None, None).await?;
page.fill("Email", "agent@example.com").await?;
page.choose("Country", "United Kingdom").await?;
page.upload("Attachment", &["/tmp/report.pdf".to_string()])
    .await?;
page.click_on("Submit", Button::Left).await?;
```

CLI:

```bash
computer browser "$BOX" find input --limit 10
computer browser "$BOX" fill "Email" "agent@example.com"
computer browser "$BOX" select "Country" "United Kingdom"
computer browser "$BOX" upload "Attachment" /tmp/report.pdf
computer browser "$BOX" click "Submit"
```

These operations use page structure. They do not depend on the position of the Chromium window.

### Keep page and screen state aligned

`open_url()` opens and raises a new tab. Coordinates from an earlier screenshot then address the new frontmost page.

Use page visibility methods before you mix DevTools operations with screen coordinates:

```rust
let mut page = browser
    .open_page("https://example.com", Duration::from_secs(20))
    .await?;

computer.open_url("https://example.net").await?;
assert!(!page.visible().await?);

page.bring_to_front().await?;
assert!(page.visible().await?);
```

CLI:

```bash
TAB=$(computer open "$BOX" https://example.com)
computer open "$BOX" https://example.net
computer browser "$BOX" tabs
computer browser "$BOX" switch "$TAB"
computer screenshot "$BOX" current-page.png --tab "$TAB"
```

`Devtools::visible_page()` returns the page that the screen currently shows.

### Isolate browser sessions

A browser group is a Chromium browser context. Groups share the Chromium process but keep cookies, local storage, IndexedDB, and service workers separate:

```rust
let group = browser.create_group().await?;
let mut page = group
    .open_page("https://example.com", Duration::from_secs(20))
    .await?;

page.evaluate("localStorage.setItem('agent', 'one')")
    .await?;

group.close().await?;
```

Groups do not create screens. Only one page can be frontmost. Call `bring_to_front()` before you use screen coordinates.

### Carry a login between desktops

Export the parts of a browser session that belong to named origins:

```rust
let origins = ["https://example.com".to_string()];
let session = browser
    .export_session(&origins, Carry::default())
    .await?;

let tabs = browser.import_session(&session).await?;
```

`Carry::default()` includes cookies and local storage. Enable IndexedDB for sites that store login state there. Session data grants account access and must be handled as a credential.

## Display and window operations



### List and arrange windows

```rust
let screen = computer.primary();
let windows = screen.windows().await?;
let active = screen.active_window().await?;

screen
    .arrange(
        &window.id,
        Arrange::Size {
            width: 800,
            height: 600,
        },
    )
    .await?;
screen
    .arrange(&window.id, Arrange::At(Point::new(120, 90)))
    .await?;
```

CLI:

```bash
computer window "$BOX" list
computer window "$BOX" active
computer window "$BOX" 42 size 800 600
computer window "$BOX" 42 move 120 90
```

Match a window by class when possible. A title can change with the open document.

Wait for a new window to appear and stop moving:

```rust
let dialog = screen
    .wait_for_window("Mousepad", Duration::from_secs(10))
    .await?;
```

CLI:

```bash
computer window "$BOX" wait Mousepad --within 10
```

The result reports the final geometry. A window manager can clamp or reject the requested size or position.

### Use more than one screen

Screen IDs start at zero. Screen 0 starts with the desktop. Other screens start when they are first requested:

```rust
let second = computer.screen(ScreenId(1)).await?;
second.open_url("https://example.org").await?;
```

The bundled image supports up to eight screens. Each screen has separate browser data, clipboard selections, and view and control servers.

`screen()` leases the screen to the current process. A second caller is refused instead of receiving the same screen.

### Change the wallpaper

```rust
computer
    .set_wallpaper(&std::fs::read("background.png")?)
    .await?;

let second = computer.screen(ScreenId(1)).await?;
second.set_wallpaper(&bytes).await?;
```

PNG and JPEG data are supported. A profile without wallpaper support returns `Unsupported`.

### Record the screen

The basic recording helper builds a GIF from screenshots. Add video packages to record MP4:

```rust
let computer = Computer::builder()
    .packages(Extras::video().packages)
    .launch()
    .await?;

computer
    .record(Duration::from_secs(10), "/tmp/screen.mp4")
    .await?;
computer
    .download("/tmp/screen.mp4", "screen.mp4")
    .await?;
```

CLI:

```bash
BOX=$(computer new --video)
computer record "$BOX" start --fps 20
computer record "$BOX" status
computer record "$BOX" stop screen.mp4
```

Add `Extras::audio()` when the recording also needs sound.

## Human control



### Hand over exclusive control

`hand_over()` gives a person exclusive input through a browser:

```rust
let takeover = computer.hand_over().await?;
let control_url = takeover.url();

let frame = computer.screenshot().await?;
assert!(computer.click(at, Button::Left).await.is_err());

takeover.end().await?;
```

CLI:

```bash
computer takeover "$BOX"
computer release "$BOX"
```

The program can continue to read the screen while the person controls it. Its input operations return an error.

Use `share()` only when the person and the program must send input at the same time:

```rust
let shared = computer.share().await?;
```

Shared input can race. Use exclusive handover when both sides do not need simultaneous control.

### Wait for the person to leave

```rust
let takeover = computer.hand_over().await?;

computer
    .wait_until_free(Duration::from_secs(600))
    .await?;
takeover.end().await?;

let new_frame = computer.screenshot().await?;
```

Always take a new screenshot after a handover.

### Reclaim a desktop

An attached process can end a takeover that outlived its owner:

```rust
let computer = Computer::attach("my-box").await?;

if computer.person_driving().await {
    computer.reclaim().await?;
}
```

CLI:

```bash
computer release "$BOX"
```

The image also refuses raw synthetic input from inside the box while a person has exclusive control.

## Configure a desktop



### Builder options

Use the builder to change the defaults:

```rust
let computer = Computer::builder()
    .size(1920, 1080)
    .network(false)
    .memory("2g")
    .runtime("podman")
    .name("my-box")
    .keep_on_drop(true)
    .launch()
    .await?;
```

CLI:

```bash
BOX=$(computer new --size 1920x1080 --no-network --memory 2g --runtime podman --ttl 60)
```

`computer new` keeps the desktop after the command exits. `--app gimp` and `--package jq` install into the image, and `--spec box.json` takes a file shaped like the body of `POST /v1/boxes` for what has no flag; a flag goes over the file.

- `network(false)` blocks outbound network access from the desktop.
- `runtime()` also accepts `nerdctl`.
- `keep_on_drop(true)` leaves the desktop running when the handle is dropped.
- `expires_after(duration)` removes the desktop after a fixed time.
- `expires_when_idle(duration)` removes it after a period without API activity.

`Computer::builder().config()` returns the resolved image, ports, environment, and boot command without starting a desktop.

### Add fonts and packages

The default image includes Latin fonts. Add wider language support or Debian packages when required:

```rust
let computer = Computer::builder()
    .wide_fonts()
    .packages(["vim", "curl"])
    .launch()
    .await?;
```

CLI:

```bash
BOX=$(computer new --wide-fonts --accessibility --video)
```

`Extras::audio()`, `Extras::video()`, `Extras::accessibility()`, and `Extras::everything()` provide common package sets.

The package list is part of the image tag. The first launch with a new list builds a new image.

### Keep browser data

Keep the complete Chromium profile in a named container volume:

```rust
let computer = Computer::builder()
    .profiles("agent-work")
    .launch()
    .await?;
```

The profile includes logins, history, extensions, and browser storage. It stays on the host and can be used by only one desktop at a time.

Use session export when the data must move between hosts. Use a named profile when all browser state must stay on one host.

### Attach to a running desktop

```rust
let computer = Computer::attach("my-box").await?;
```

CLI:

```bash
computer ls
computer screenshot "$BOX" screen.png
```

CLI commands attach to the desktop named by their `<box>` argument.

The attached desktop keeps its windows, browser profile, and files. Dropping an attached handle does not remove a desktop that the handle did not create.

## Runtimes

The desktop operations stay the same across runtimes. Startup, isolation, image storage, networking, and cleanup can differ.

### Containers

Docker is the default. Podman and nerdctl use the same image and desktop API:

```rust
let computer = Computer::builder()
    .runtime("podman")
    .launch()
    .await?;
```

A container shares the host kernel. The runtime selects free host ports, and the desktop is removed when its owning handle is dropped unless configured otherwise.

### microVMs

A microVM boots its own kernel and gives a stronger isolation boundary. It starts more slowly and uses a separate image store.

The included integration targets microsandbox 0.6. Import the image into the hypervisor before the first launch. See `[examples/microvm.rs](examples/microvm.rs)` for the complete flow.

Other hypervisors can implement `MicroVmApi`.

### Cloud sandboxes

The included E2B integration runs the desktop away from the local host:

```toml
computer = { git = "https://github.com/CITGuru/computer", default-features = false, features = ["e2b"] }
```

```bash
export E2B_API_KEY=...
cargo run --features e2b --example e2b -- <template-id>
```

E2B uses templates instead of local container images. Its viewer can be public, so public viewer access requires authentication.

Remote cloud profiles do not expose Chrome DevTools. Screen input, screenshots, clipboard operations, human control, and file transfer remain available.

### X11 and Wayland

X11 is the default display profile. Select the Wayland profile explicitly:

```rust
let computer = Computer::builder()
    .profile(Arc::new(WaylandProfile))
    .launch()
    .await?;
```

CLI:

```bash
BOX=$(computer new --wayland)
```

The public desktop API is the same for both profiles.

The X11 image uses Xvfb, fluxbox, x11vnc, ImageMagick, and `xdotool`. The Wayland image uses headless sway, wayvnc, `grim`, and one virtual pointer and keyboard per screen that stay for the life of the screen, so a button or a modifier can be held between two steps as on X11.

Wayland cannot read the global pointer after a person moves it. `cursor()` returns `Unsupported` after a handover until the driver moves the pointer again.

## Custom desktops and runtimes



### Use a custom image

Build a local Docker context:

```rust
let computer = Computer::builder()
    .image_dir("images/my-desktop")
    .launch()
    .await?;
```

Or pull an existing image:

```rust
let computer = Computer::builder()
    .image("registry.example.com/desktop:1")
    .launch()
    .await?;
```

A local context must contain a `Dockerfile` that implements the selected profile. A registry image cannot be combined with extra packages because the crate does not build that image.

An image can declare its contract:

```dockerfile
LABEL computer.profile="computer-desktop"
```

The crate refuses a declared profile mismatch before startup.

See `[examples/custom_image.rs](examples/custom_image.rs)` and `[examples/images/acme/](examples/images/acme/)` for a small derived image.

### Define a desktop profile

Use `ProfileBuilder` when an image keeps most of a shipped contract:

```rust
let profile = ProfileBuilder::new(X11Profile)
    .name("my-desktop")
    .image_dir("images/mine")
    .screen_commands(CommandScreen::new("my-screen"))
    .wallpaper_runtime(CommandWallpaperRuntime::new("my-wallpaper"))
    .build();

let computer = Computer::builder()
    .profile(Arc::new(profile))
    .launch()
    .await?;
```

A profile defines the image contract, ports, geometry, environment, capabilities, and display driver. A new display server implements `Desktop` and `DesktopFactory`.

### Add a cloud sandbox vendor

`RemoteApi` is the common interface for E2B, Daytona, Modal, and similar services. An adapter creates, finds, and removes a sandbox; runs commands; and reads and writes files.

`RemoteMachine` supplies shared lifetime, naming, and keep-alive behavior. `RemoteProfile` maps vendor endpoints and removes capabilities that the remote service cannot expose.

Run the local reference adapter before you connect an external API:

```bash
cargo run --example custom_sandbox
```

See `[examples/custom_sandbox.rs](examples/custom_sandbox.rs)` for a complete adapter and `computer::testing::ScriptedRemote` for tests without an account or network.

### Audit a desktop

`DesktopSupport` states what a profile provides. `audit` tests those claims against a running desktop:

```rust
let audit = computer::audit(&computer).await;
assert!(audit.ok());
```

The audit checks the screen, pointer, DevTools connection, clipboard, viewer, and handover where the profile claims support.

### Clean up expired desktops

```rust
let removed = computer::sweep_expired(
    &DockerMachine::default(),
    SystemTime::now(),
)
.await?;
```

CLI:

```bash
computer sweep
```

Use deadlines for services that can stop before normal shutdown.

## What is inside the box

The default image is based on `debian:bookworm-slim`. It includes:

- Xvfb and fluxbox for the default virtual desktop
- Chromium with a separate profile for each screen
- x11vnc, websockify, and noVNC for browser viewing
- `xdotool` for pointer and keyboard input
- ImageMagick for PNG capture
- `xclip` for clipboard and primary selections
- `socat` for the Chrome DevTools bridge
- an input guard that blocks synthetic input during human control

The image tag contains a hash of its source files. A source change creates a new tag instead of reusing stale image contents.

## Security

The local viewer is open by default and is published only on loopback. Anyone who can reach a control port can drive the desktop.

Publishing outside loopback requires authentication:

```rust
let computer = Computer::builder()
    .auth(Auth::Token)
    .publish_on(Bind::Any)
    .advertise("boxes.example.com")
    .launch()
    .await?;
```

`Auth::Password` prompts in the browser and keeps the password out of the URL. `Auth::Token` puts a ticket in the URL so one link carries access.

Watch and control viewers use separate credentials. Changing the port on a watch URL does not create control access.

Chrome DevTools is not published outside the host because CDP has no authentication. Use a tunnel or connect from inside the box.

`network(false)` blocks network access from the desktop. It does not protect the viewer.

A control port exists only during human handover. While it is open, the box rejects other synthetic input.

Browser sessions and persistent profiles contain authenticated account state. Treat them as credentials.

## Examples

Start with these:

- `[quickstart](examples/quickstart.rs)` launches, controls, and captures a desktop.
- `[browser](examples/browser.rs)` controls Chromium through DevTools.
- `[elements](examples/elements.rs)` finds and acts on page elements.
- `[capture](examples/capture.rs)` captures regions and windows at different sizes.
- `[takeover](examples/takeover.rs)` gives control to a person and takes it back.
- `[tour](examples/tour.rs)` controls two screens.

The repository also includes:

- `[serve](examples/serve.rs)` leaves a named desktop running.
- `[attach](examples/attach.rs)` connects to a running desktop.
- `[recording](examples/recording.rs)` creates an animated GIF.
- `[waiting](examples/waiting.rs)` waits for browser state.
- `[research](examples/research.rs)` searches and reads web pages.
- `[from_spec](examples/from_spec.rs)` launches from a portable specification.
- `[custom_image](examples/custom_image.rs)` builds and uses another image.
- `[microvm](examples/microvm.rs)` runs the desktop with microsandbox.
- `[custom_sandbox](examples/custom_sandbox.rs)` implements a sandbox vendor.
- `[e2b](examples/e2b.rs)` runs in an E2B sandbox.
- `[e2b_takeover](examples/e2b_takeover.rs)` gives an E2B desktop to a person.

Run an example with Cargo:

```bash
cargo run --example quickstart
```

## Development and testing

The normal test suite does not need a container runtime:

```bash
cargo test --workspace --no-fail-fast
```

Live tests are ignored by default:

```bash
cargo test --test live -- --ignored --nocapture
cargo test --test live_microvm -- --ignored --nocapture
cargo test --test live_extras -- --ignored --nocapture
cargo test --features e2b --test live_e2b -- --ignored --nocapture
```

`computer::testing` provides test doubles for screens, container runtimes, hypervisors, and remote sandboxes.

## License

MIT. See [LICENSE](LICENSE).