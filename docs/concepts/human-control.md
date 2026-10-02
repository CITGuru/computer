# Human control

A person can watch a box or take control of it from a browser. Use this when an agent needs help: a login, a CAPTCHA, a payment, or a decision that the agent must not make alone.

## Viewers

Each screen has two viewers. Both are noVNC pages in a browser.

| Viewer | Input | Available |
| --- | --- | --- |
| Watch | None. The VNC server refuses input. | Always |
| Control | Mouse and keyboard | Only during a takeover |

The two viewers use different ports and different credentials. A watch URL does not give control access.

Get the watch URL:

| Interface | How |
| --- | --- |
| CLI | `holm new` prints it on standard error. `holm box` shows it. |
| Rust | `computer.viewer_url()` |
| MCP | `launch_box` and `inspect_box` return it. |
| REST | `viewer_url` in `GET /v1/boxes/{id}` |

### Signed viewers

A box on a cloud runtime is reached through a public vendor URL. When the server has a secret key (`HOLM_SERVER_SECRET_KEY` or `HOLM_SERVER_SECRET_FILE`), the server gives each such box its own viewer key, and the box accepts only short-lived tokens signed with that key.

- `viewer_url` is a new link in each reply, and it must be opened in 15 minutes. Read the box again to get a new one. A box that the vendor does not open to the public has no `viewer_url`.
- `POST /v1/boxes/{id}/screens/{screen}/viewer/ticket` returns a short-lived token for the server's viewer socket, and direct socket URLs in `view_socket` and `control_socket`. A direct URL must be opened before the token expires.
- A takeover returns a control URL that can be opened for half of the lifetime of the box. For a box with no end time, that is half of the default lifetime.
- A token names the viewer that it opens. A token for the watch viewer reaches only the watch viewer, on each of the two ports, so it does not give control.
- The server keeps no token in memory. Any server with the same secret key accepts it.

Without a secret key, such a box gets a fixed token in its URL, as before.

## Take control

A takeover gives a person the control viewer. There are two kinds.

| Kind | Agent input | Use it when |
| --- | --- | --- |
| Exclusive (default) | Refused until the takeover ends | The person does a task alone |
| Shared | Accepted | The person and the agent must both send input |

Shared input can conflict. Use exclusive unless both sides must act at the same time.

During an exclusive takeover, the agent can still read the screen, take screenshots, and read pages. Its input actions fail. Inside the box, `xdotool` on the `PATH` is a wrapper that refuses input actions during the takeover, so `run_command` cannot send input with it. A program that connects to the display server directly is not stopped.

### CLI

```bash
holm takeover "$BOX"
holm release "$BOX"
```

`takeover` prints the control URL. `release` ends the takeover and shows how many people watch and drive.

In the control viewer, Cmd+V or Ctrl+V pastes the person's local clipboard into the box. The browser can ask the person for permission to read the clipboard.

### MCP

| Tool | Effect |
| --- | --- |
| `hand_over` | Starts an exclusive takeover and returns the control URL. |
| `reclaim_screen` | Ends the takeover. |
| `screen_status` | Shows who controls the screen and whether it is recording. |
| `open_screen` | Shows the live screen in the host, with buttons to take control and to record. It does not start a takeover. |

### Rust

```rust
let takeover = computer.hand_over().await?;
println!("{}", takeover.url().unwrap_or_default());

computer.wait_until_free(Duration::from_secs(600)).await?;
takeover.end().await?;

let frame = computer.screenshot().await?;
```

`share()` starts a shared takeover. `wait_until_free()` returns when no person has the control viewer open. It counts live connections.

### REST

```bash
curl -X POST "$BASE/v1/boxes/$BOX/screens/0/takeover" -d '{"shared": false}' -H 'content-type: application/json'
curl "$BASE/v1/boxes/$BOX/screens/0/viewers"
curl -X DELETE "$BASE/v1/boxes/$BOX/screens/0/takeover"
```

`viewers` returns how many people watch, how many drive, and whether a takeover is active.

## After a takeover

- **Take a new screenshot.** The person can change anything on the screen.
- **Buttons and keys are released.** When a takeover starts, the server releases each button and key that the agent held down.
- **The trace records it.** `takeover_started` and `takeover_ended` mark the takeover. The server captures a frame before it starts and another before it ends, so the trace shows what the person left.
- **On Wayland**, the box cannot read the pointer position after a person moves it. `cursor` returns `Unsupported` until the agent moves the pointer.

## End a takeover that has no owner

If the process that started a takeover stops, the takeover continues. Another process can end it:

```rust
let computer = Computer::attach("my-box").await?;
if computer.person_driving().await {
    computer.reclaim().await?;
}
```

With a server, `holm release` or `reclaim_screen` ends it from any client.

## The live screen in an MCP host

Hosts that support [MCP Apps](https://github.com/modelcontextprotocol/ext-apps) show the live screen next to the results of `launch_box`, `open_screen`, and `hand_over`. A person can watch, take control, give control back, and record.

The page connects to the box through `holmd`, not to the box's own ports, so the box can stay on loopback. Each tool result gives the page a short-lived token for one screen. The token is valid for 15 minutes and goes to the page only. The model does not get it.

When `holmd` is behind a reverse proxy, set `HOLM_PUBLIC_URL` and forward WebSocket upgrades. See [The server](server.md#behind-a-reverse-proxy).

## Viewer access

By default, a box publishes its viewers only on the host's loopback address, with no password. Only processes on the same host can reach them.

To publish viewers on other addresses, the box must have a credential. The library refuses to launch a box that publishes beyond loopback with an open viewer.

| `Auth` | Credential | How the person gets access |
| --- | --- | --- |
| `Open` (default) | None | Loopback only |
| `Password` | A password for each viewer | The browser asks for it. It is not in the URL. |
| `Token` | A token for each viewer | The token is in the URL. |

```rust
let computer = Computer::builder()
    .auth(Auth::Token)
    .publish_on(Bind::Any)
    .advertise("boxes.example.com")
    .launch()
    .await?;
```

In a box spec, the same settings are `policy.auth`, `policy.bind`, and `policy.advertise`.

Choose `Password` when a link can be copied to other places, such as chat or logs. Choose `Token` when one link must carry access, for example from a service that makes a link for each person.

Treat viewer URLs as credentials. A control URL gives full control of the desktop.

Chrome DevTools has no authentication of its own. On host runtimes its port stays on loopback. On E2B and Vercel, the bridge in the box refuses requests without the box's secret. Use `holm cdp` for an address with a short-lived token through the server.
