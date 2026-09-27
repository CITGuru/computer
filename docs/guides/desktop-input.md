# Desktop input

This guide sends mouse and keyboard input to a box by screen coordinates, captures the screen, and uses the clipboard. It works with any application. On a web page, act on elements by name instead: see [Control modes](../concepts/control-modes.md).

## Coordinates

Coordinates are device pixels on one screen. `(0, 0)` is the top-left corner.

- Get a point from the most recent full-size screenshot.
- Do not calculate a point from a scaled or cropped capture.
- Take a new screenshot after a person, another process, or a new browser tab changes the screen.
- A screenshot does not show the pointer. Use `cursor` to get its position.

## Mouse

| Operation | CLI | MCP | Rust |
| --- | --- | --- | --- |
| Move | `mouse <box> move X Y` | `batch` step `move` | `move_to(at)` |
| Click | `mouse <box> click X Y [button]` | `click` | `click(at, button)` |
| Double click | `mouse <box> click X Y --double` | `click` with `double` | `double_click(at, button)` |
| Drag | `mouse <box> drag X1 Y1 X2 Y2` | `drag` | `drag(from, to, button)` |
| Path | `mouse <box> path X1 Y1 X2 Y2 …` | `batch` step `draw` | `drag_along(from, &steps, button, &held)` |
| Button down | `mouse <box> down [X Y]` | `mouse_down` | `button_down(at, button)` |
| Button up | `mouse <box> up [X Y]` | `mouse_up` | `button_up(at, button)` |
| Pointer position | `mouse <box> at` | `cursor` | `cursor()` |

`button` is `left` (default), `right`, or `middle`.

In Rust, `move_along`, `drag_along`, `button_down`, `button_up`, `key_down`, and `key_up` are methods of the `Desktop` trait. Import it with `use computer::Desktop as _;`.

```bash
computer mouse "$BOX" move 640 400
computer mouse "$BOX" click 640 400
computer mouse "$BOX" click 640 400 right
computer mouse "$BOX" click 640 400 left --double
computer mouse "$BOX" drag 100 100 400 300
```

```rust
computer.move_to((640, 400)).await?;
computer.click((640, 400), Button::Left).await?;
computer.double_click((640, 400), Button::Left).await?;
computer.drag((100, 100), (400, 300), Button::Left).await?;
```

A drag is one press, one move, and one release. A path is one press, a move through each point, and one release, so it draws one stroke with corners. A drag for each part of the line draws separate strokes.

```bash
computer mouse "$BOX" path 100 100 300 100 300 300 100 300
```

### Modifier keys

Hold modifier keys during a click, drag, or path with `--held`. Pressing a key in a separate command does not work, because that key is released when its command ends.

```bash
computer mouse "$BOX" click 640 400 left --held shift
computer mouse "$BOX" drag 100 100 400 300 left --held ctrl
```

```rust
computer.click_with((640, 400), Button::Left, &[Held::Shift]).await?;
computer.drag_with((100, 100), (400, 300), Button::Left, &[Held::Ctrl]).await?;
```

MCP: `held: ["shift"]` on `click` or `drag`. The modifiers are `shift`, `ctrl`, `alt`, and `super`.

### Motion

The pointer jumps to a point unless you ask for motion:

| Motion | Movement |
| --- | --- |
| `instant` (default) | Jumps. |
| `smooth` | Moves on a straight line. |
| `human` | Moves on a curve, over 100 to 700 ms by distance. The same seed gives the same curve. |

Use `human` for a page that watches pointer movement, or for a person who watches the screen.

```bash
computer mouse "$BOX" move 640 400 --smooth
computer mouse "$BOX" click 640 400 left --human --seed 42
```

MCP: `motion: "human"` and `seed: 42` on `click`, `drag`, `mouse_down`, and the page tools.

```rust
use computer::{Desktop as _, Motion, motion};

let steps = motion::path(Point::new(100, 100), Point::new(400, 300), Motion::Human, 42);
computer.move_along(&steps).await?;
```

### Hold a button

Use `down` and `up` only when a drag cannot do the task, for example when you must wait for something before you release:

```bash
computer mouse "$BOX" down 400 400 left --hold 10
computer mouse "$BOX" move 900 600 --smooth
computer mouse "$BOX" up 900 600 left
```

With no point, `down` and `up` act at the pointer. A move between them drags.

The server releases a button that stays down:

- After `--hold` seconds (`hold_seconds` in MCP, `hold_ms` in REST). Default 10 seconds, maximum 60.
- When a person takes control.
- At the end of a batch, unless the step gave a hold time. The result names each released button.

`down` and `up` need a server. They do not work with `--local`.

## Keyboard

| Operation | CLI | MCP | Rust |
| --- | --- | --- | --- |
| Type text | `keyboard <box> type TEXT` | `type_text` | `type_text(text)` |
| Press keys | `keyboard <box> press KEYS…` | `press_key` | `press(keys)` |
| Key down | `keyboard <box> down KEY` | `key_down` | `key_down(key)` |
| Key up | `keyboard <box> up KEY` | `key_up` | `key_up(key)` |

Typing goes to the element that has the keyboard focus. Click the field, or focus it, first.

```bash
computer keyboard "$BOX" type "hello"
computer keyboard "$BOX" press enter
computer keyboard "$BOX" press ctrl+l
```

```rust
computer.type_text("hello").await?;
computer.press("enter").await?;
computer.press("ctrl+l").await?;
```

### Slow typing

Some inputs act on each key and lose characters that arrive at full speed. Add a delay between keys. 20 to 50 ms is usually enough.

```bash
computer keyboard "$BOX" type "one key at a time" --delay 30
```

```rust
computer.type_text("one key at a time").every(Duration::from_millis(30)).await?;
```

MCP: `delay_ms: 30` on `type_text`. REST: `delay_ms` on a `type` action.

### Key names

A key is a name or a combination: `enter`, `tab`, `escape`, `up`, `pagedown`, `ctrl+a`, `cmd+shift+p`. Common other names also work, such as `esc`, `return`, `pgdn`, `cmd`, and `win`.

### Hold a modifier across keys

A combination releases its modifiers at the end. To keep a modifier down across several keys, give all the keys in one command with `--held`:

```bash
computer keyboard "$BOX" press tab tab tab --held alt
```

```rust
computer.press(["tab", "tab", "tab"]).holding([Held::Alt]).await?;
```

MCP: `press_key` with `chord: "tab"`, `then: ["tab", "tab"]`, and `held: ["alt"]`.

This goes to the third window. Three separate `alt+tab` presses go only to the second.

### Hold a key

`down` keeps one key down until `up`. Everything typed or clicked while it is down carries it:

```bash
computer keyboard "$BOX" down shift --hold 20
computer mouse "$BOX" click 300 200
computer mouse "$BOX" click 300 400
computer keyboard "$BOX" up shift
```

`down` takes one key, not a combination. The server releases the key at the same times as a button.

## Scroll

`scroll` turns the mouse wheel at a point, in notches. It moves what is under the point, so it reaches a list or a sidebar without a click.

```bash
computer mouse "$BOX" scroll 640 400 down 3
computer mouse "$BOX" scroll down
computer mouse "$BOX" scroll 640 400 5 -2
```

With no point, the CLI scrolls at the center of the screen. With no count, it scrolls 3 notches. The signed form takes `DY` then `DX`: positive is down and right.

```rust
computer.scroll((640, 400), Delta::down(3)).await?;
computer.scroll((640, 400), Delta::right(3)).await?;
```

MCP: `scroll` with `x`, `y`, `dy`, and `dx`.

On a web page, `scroll_page` (MCP) moves the page by pixels and returns the new position. See [Control modes](../concepts/control-modes.md#browser).

## Wait for the screen

After an action that draws, such as opening a menu, wait until the screen stops changing before you take a screenshot:

```bash
computer wait "$BOX" --settle 400 --within 10000
```

```rust
computer.wait_until_still(Duration::from_millis(400), Duration::from_secs(10)).await?;
```

MCP: `wait_until_still`. REST: a `wait_still` action.

The screen must not change for `settle` (default 400 ms). The wait stops at `within` (default 10 seconds). A screen with an animation reaches the limit. On a web page, `wait_for` is better: it waits for the element you need.

## Screenshots

| Capture | CLI | MCP (`screenshot`) | Rust |
| --- | --- | --- | --- |
| Full screen | `screenshot <box> out.png` | no parameters | `screenshot()` |
| One window | `--window ID` | `window` | `capture(&Shot::window(id))` |
| A rectangle | `--at X,Y --size WxH` | `x`, `y`, `width`, `height` | `capture(&Shot::region(rect))` |
| Smaller | `--scale PERCENT` | `scale` | `.scaled(percent)` |
| With the pointer | `--pointer` | `pointer` | `Shot { pointer: true, .. }` |

```bash
computer screenshot "$BOX" full.png
computer screenshot "$BOX" region.png --at 100,80 --size 400x300
computer screenshot "$BOX" window.png --window 42
computer screenshot "$BOX" small.png --scale 50
```

```rust
let full = computer.screenshot().await?;
let region = computer
    .capture(&Shot::region(Rect::new(Point::new(100, 80), 400, 300)))
    .await?;
let small = computer.capture(&Shot::of(Of::Screen).scaled(50)).await?;
```

Get window IDs from `computer window <box> list` or the MCP `window` tool with `op: list`. A window capture uses the window's position when the capture runs.

A scaled image uses fewer bytes, but do not calculate click points from it.

MCP tools that change the screen return a frame. Give `have_frame` with the last frame's hash: if the screen did not change, the result says `unchanged` and sends no image.

## Clipboard

Each screen has two selections:

| Selection | Filled by | Pasted by |
| --- | --- | --- |
| `clipboard` (default) | Copy | Paste (`ctrl+v`) |
| `primary` | Selecting text | A middle click |

```bash
computer clip "$BOX" "ready to paste"
computer clip "$BOX"
computer clip "$BOX" "middle-click paste" --primary
computer clip "$BOX" --primary
```

```rust
computer.set_clipboard("ready to paste").await?;
let text = computer.clipboard().await?;
computer.set_selection(Selection::Primary, "middle-click paste").await?;
```

MCP: `clipboard` with `text` to set it, or without `text` to read it, and `selection`. REST: `GET` and `PUT /v1/boxes/{id}/screens/{screen}/clipboard`.

Setting the clipboard and pasting is faster than typing long text. From Rust, `clipboard_bytes` and `set_clipboard_bytes` move other types, such as `image/png`.

## Batches

A batch runs several steps in one call. The box holds the screen for all the steps, so no other client can act between them, and one result comes back at the end. Use a batch when you know the steps in advance: a form, a drawing, or a key held across clicks.

### CLI

`computer batch` reads a file, or standard input, with a list of REST actions:

```json
[
  { "type": "click", "at": { "x": 200, "y": 150 } },
  { "type": "type", "text": "hello", "delay_ms": 20 },
  { "type": "press", "chord": "enter" },
  { "type": "wait_still", "settle_ms": 400 }
]
```

```bash
computer batch "$BOX" steps.json --settle 400
```

The file can also be an object with an `actions` list. `--keep-going` runs the remaining steps after a step is refused.

### MCP

`batch` takes `actions`, each with a tool name and that tool's arguments:

```json
{
  "box_id": "box_…",
  "actions": [
    { "tool": "mouse_down", "arguments": { "x": 100, "y": 100 } },
    { "tool": "move", "arguments": { "x": 200, "y": 150, "pause_ms": 40 } },
    { "tool": "move", "arguments": { "x": 300, "y": 100, "pause_ms": 40 } },
    { "tool": "mouse_up", "arguments": {} }
  ],
  "keep_going": true
}
```

`move` and `draw` are steps only in a batch. `draw` takes `through`, a list of `{x, y}`. `pause_ms` on `move` waits after the move. A drawing program needs about 40 ms to see each point.

### REST

`POST /v1/boxes/{id}/screens/{screen}/actions` takes the same list as the CLI, under `actions`. See [Actions](../reference/rest-api.md#actions) for every action type.

### Buttons and keys at the end

A button or key that is still down when the batch ends is released, and the result names it in `released` or `released_keys`. A step that gives a hold time keeps it down until that time, and the result lists it in `holding` or `holding_keys`.
