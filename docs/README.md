# Computer documentation

`computer` gives agents isolated Linux desktops. Each box runs in a container, a microVM, or a cloud sandbox, and you drive it from the CLI, from Rust, over REST, or over MCP.

## Getting started

- [Install](getting-started/install.md) — requirements, the install script, and a build from source
- [CLI quick start](getting-started/quickstart-cli.md) — open a box, drive it, and remove it from a shell
- [Rust quick start](getting-started/quickstart-rust.md) — launch a desktop from a Rust program
- [MCP quick start](getting-started/quickstart-mcp.md) — give an MCP host such as Claude Code or Cursor a desktop

## Concepts

- [Boxes](concepts/boxes.md) — the spec, screens, states, lifetime, traces, forks, and what is inside a box
- [Control modes](concepts/control-modes.md) — screen coordinates, accessibility, or the browser, and when to use each
- [The server](concepts/server.md) — `computer` and `computerd`, how a client finds a server, access control, and storage
- [Human control](concepts/human-control.md) — viewers, handing control to a person, taking it back, and viewer access
- [Runtimes](concepts/runtimes.md) — where a box runs: containers, microVMs, and cloud sandboxes

## Guides

- [Desktop input](guides/desktop-input.md) — mouse, keyboard, scrolling, waits, screenshots, clipboard, and batches
- [Files and commands](guides/files-and-commands.md) — run commands, and move files in and out of a box
- [Accessibility](guides/accessibility.md) — drive native windows by widget name
- [Browser](guides/browser.md) — tabs, snapshots, page actions, waits, logins, and other CDP libraries
- [Windows and screens](guides/windows-and-screens.md) — windows, applications, more than one screen, and recording
- [Configure a box](guides/configure-a-box.md) — screens, applications, packages, fonts, network, browser data, and spec files
- [Custom images](guides/custom-images.md) — your own desktop image and profile, and how to audit it
- [Cloud sandboxes](guides/cloud-sandboxes.md) — run boxes on E2B, and add another vendor
- [Vercel Sandbox](guides/vercel-sandbox.md) — run boxes on Vercel, with the image built there
- [Daytona](guides/daytona.md) — run boxes on Daytona, with the image built there
- [Modal](guides/modal.md) — run boxes on Modal, with the image built there
- [Trace and fork](guides/trace-and-fork.md) — read what happened to a box, and make a new box from it
- [Deploy computerd](guides/deploy-computerd.md) — run the server as a service behind TLS, and connect clients to it

## Reference

- [CLI](reference/cli.md) — every `computer` command, flag, and environment variable
- [REST API](reference/rest-api.md) — every `/v1` endpoint, the box spec, and the action types
- [MCP tools](reference/mcp-tools.md) — every MCP tool and its parameters
- [Rust API](reference/rust-api.md) — the main types, methods, errors, extension points, and examples
- [Configuration](reference/configuration.md) — every environment variable, file field, cargo feature, and box setting

## More

- [Examples](examples.md) — every example and demo, and how to run it
- [Contributing](../CONTRIBUTING.md) — build, test, lint, and the workspace crates
