# Windows, screens, and recording

This guide works with the windows on a desktop, opens applications, uses more than one screen, changes the wallpaper, and records the screen.

## Windows

### List windows

```bash
holm window "$BOX" list
holm window "$BOX" active
```

Each window has an `id`, `title`, `class`, position (`at`), `width`, and `height`. Use the `id` in later commands. `active` returns the window that receives keyboard input.

### Focus, close, and arrange

```bash
holm window "$BOX" "$WIN" focus
holm window "$BOX" "$WIN" move 120 90
holm window "$BOX" "$WIN" size 800 600
holm window "$BOX" "$WIN" max
holm window "$BOX" "$WIN" min
holm window "$BOX" "$WIN" restore
holm window "$BOX" "$WIN" close
```

Use an `id` from `list`. `close` asks the application to close the window, and the application can ask a question first.

Move, size, and state changes return the final geometry. The window manager can change a request, for example to keep a window on the screen or to respect the application's size limits. On Wayland, minimize moves the window to the sway scratchpad.

### Wait for a window

After an action that opens a window, wait for it. Do not use a fixed delay.

```bash
holm window "$BOX" wait Mousepad --within 10
```

The command returns when a window of that class appears and stops moving. Use the class, not the title: a title often changes with the open document.

The CLI takes `--within` in seconds. REST and MCP take `within_ms` in milliseconds. The default is 30 seconds.

### The same steps in other interfaces

| Task | MCP (`window` tool) | REST | Rust (`computer.primary()`) |
| --- | --- | --- | --- |
| List | `op: "list"` | `GET …/screens/{n}/windows` | `windows()` |
| Active | `op: "active"` | `GET …/screens/{n}/windows/active` | `active_window()` |
| Wait | `op: "wait"`, `class`, `within_ms` | `POST …/screens/{n}/windows/wait` | `wait_for_window(class, within)` |
| Focus | `op: "focus"`, `window` | `POST …/windows/{id}/focus` | `focus(id)` |
| Close | `op: "close"`, `window` | `DELETE …/windows/{id}` | `close_window(id)` |
| Arrange | `op: "arrange"`, `window`, `how`, `x`, `y`, `width`, `height` | `POST …/windows/{id}/arrange` | `arrange(id, Arrange)` |

REST `arrange` bodies are in the [REST reference](../reference/rest-api.md#windows).

```rust
let screen = computer.primary();
let window = screen.wait_for_window("Mousepad", Duration::from_secs(10)).await?;

screen.arrange(&window.id, Arrange::Size { width: 800, height: 600 }).await?;
screen.arrange(&window.id, Arrange::At { to: Point::new(120, 90) }).await?;
screen.arrange(&window.id, Arrange::Maximise).await?;
screen.focus(&window.id).await?;
```

## Applications

Open an application that the box was launched with:

```bash
BOX=$(holm new --app text-editor)
holm app "$BOX" text-editor
holm app "$BOX" text-editor /tmp/notes.txt
```

`app` returns when the application's window has appeared and stopped changing, so the next screenshot shows it ready. Arguments after the name go to the application, such as a file to open.

| Interface | How |
| --- | --- |
| CLI | `holm app <box> <name> [args…]`, and `holm apps` for the names |
| MCP | `open_app` with `app` and `args`, and `list_apps` for the names |
| REST | The `launch` action in a batch: `{"type": "launch", "app": "text-editor", "args": []}` |
| Rust | `screen.launch(&Launch { command, class, settle, within })` with the command and window class |

To install applications, see [Configure a box](configure-a-box.md#applications).

## More than one screen

A box can have up to eight screens. Ask for them when you create the box:

```bash
BOX=$(holm new --screens 2)
```

Each screen has its own display, browser, clipboard, and viewer. Screen 0 is the primary screen.

| Interface | Screens it can use |
| --- | --- |
| REST | Any screen of the box: the `{screen}` part of each `/screens/{screen}/…` path. |
| Rust | Any screen, up to eight: `computer.screen(ScreenId(1))`. |
| CLI | Screen 0 only. |
| MCP | Screen 0 only. |

```bash
curl -s "$BASE/v1/boxes/$BOX/screens/1/actions" \
  -H 'content-type: application/json' \
  -d '{"actions": [{"type": "open_url", "url": "https://example.org"}], "want": ["frame"]}'
```

REST refuses a screen number that is not less than the box's `screens` value.

```rust
let second = computer.screen(ScreenId(1)).await?;
second.open_url("https://example.org").await?;
let png = second.screenshot().await?;
```

In Rust, a screen after screen 0 starts when you first ask for it. `screen()` leases the screen to your process, so a second caller is refused, not given the same screen.

## Wallpaper

Only the Rust API can change the wallpaper:

```rust
let image = std::fs::read("background.png")?;
computer.set_wallpaper(&image).await?;
computer.screen(ScreenId(1)).await?.set_wallpaper(&image).await?;
```

Give the bytes of an image file. An empty image is refused.

## Record the screen

Recording needs the `video` feature, which adds ffmpeg. A box has it by default. The recording is an MP4 file in the box, so the frames do not go over the network while it records.

```bash
BOX=$(holm new)
holm record "$BOX" start --fps 12
holm record "$BOX" status
holm record "$BOX" stop screen.mp4
```

- `start` fails if the screen is already recording.
- `stop` stops ffmpeg and copies the file to the host. With no file name, it writes `recording.mp4`.
- The file in the box is `/tmp/computer/recording-<screen>.mp4`. The next `start` on the same screen deletes it.
- On X11, if the box has the `audio` feature, the recording includes the screen's sound. On Wayland, the recording has no sound, and the frames come from repeated screenshots.
- The default is 12 frames per second.

| Interface | Start | Stop | Get the file |
| --- | --- | --- | --- |
| CLI | `record start --fps N` | `record stop [file]` | `stop` copies it. |
| MCP | `record` with `op: "start"`, `fps` | `record` with `op: "stop"` | Not possible. The tool gives the path in the box, for the person to collect. |
| REST | `POST …/screens/{n}/recording` with `{"fps": 12}` | `DELETE …/screens/{n}/recording` | `GET /v1/boxes/{id}/files?path=…` returns it as base64. |
| Rust | `start_recording(Some(12))` | `stop_recording()` returns the path | `download(path, local)` |

MCP `read_file` refuses files that are not UTF-8 text, so an MCP agent cannot read the video. Give the path to the person.

```rust
computer.start_recording(Some(12)).await?;
// drive the box
let inside = computer.stop_recording().await?;
computer.download(&inside, "screen.mp4").await?;
```

Rust also has `record(duration, path)`, which records for a fixed time. It uses X11 capture, so it does not work on Wayland. Use `start_recording` and `stop_recording` on Wayland.

For an animated GIF made from screenshots, see `examples/recording.rs`.
