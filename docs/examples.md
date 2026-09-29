# Examples

The repository has Rust examples in `examples/` and two larger demos in `demos/`. Run each Rust example from the repository root with `cargo run --example <name>`.

All examples need a container runtime, such as Docker, unless the table says otherwise. The first box builds the image, which takes a few minutes.

Some examples take the name of a box that is already running. Start one with `cargo run --example serve`, or use a box ID from `computer ls`.

## Getting started

| Example | Shows | Run | Needs |
| --- | --- | --- | --- |
| `quickstart` | Launch a box, open a page, click, type, take a screenshot, and read the pointer position. | `cargo run --example quickstart` | Nothing more |
| `serve` | Launch a box that stays after the program exits, and print its viewer and DevTools URLs. Options select Wayland, a dock, or a local image directory. | `cargo run --example serve -- https://example.com` | Nothing more |
| `attach` | Attach to a running box by name, type into it, and take a screenshot. | `cargo run --example attach -- <box-name> <text>` | A running box |
| `tour` | Open a box, drive it across two screens, and keep it. | `cargo run --example tour`, or `-- <box-name>` | Nothing more |
| `live_desktop` | Launch a box and check that its screen size and screenshots match what the box reports. | `cargo run --example live_desktop` | Nothing more |
| `from_spec` | Launch a box from a spec file. | `cargo run --example from_spec -- examples/box.json` | Nothing more |

`examples/box.json` is a sample spec and placement for `from_spec` and for `computer new --spec`.

## Browser

| Example | Shows | Run | Needs |
| --- | --- | --- | --- |
| `browser` | Page operations through Chrome DevTools. | `cargo run --example browser`, or `-- <box-name>` | Nothing more |
| `elements` | Find elements, fill a field by its label, use a dropdown, and fill a file input. | `cargo run --example elements -- <box>` | A running box, and a page at `/tmp/form.html` in the box |
| `waiting` | Page waits: an element that arrives late, an element that never comes, and a hover menu. | `cargo run --example waiting -- <box>` | A running box, and a page at `/tmp/wait.html` in the box |
| `research` | Search the web for a subject and read the result pages. | `cargo run --example research -- <box> "a subject"` | A running box with network access |

`elements` and `waiting` open fixture pages that the examples do not write. Put your own pages at those paths with `computer file "$BOX" put`.

## Desktop

| Example | Shows | Run | Needs |
| --- | --- | --- | --- |
| `capture` | Screenshots of the full screen, a rectangle, and a window. | `cargo run --example capture -- <box>` | A running box |
| `recording` | Take frames while a box works, and make a GIF from them in the box. | `cargo run --example recording -- out.gif` | Nothing more |
| `demo` | The README animation: a page, typed text, a text selection by drag, a context menu, a paste, and a second screen. | `cargo run --example demo -- media/demo.gif` | Nothing more |

## Human control

| Example | Shows | Run | Needs |
| --- | --- | --- | --- |
| `takeover` | Give the screen to a person, wait, and take it back. | `cargo run --example takeover`, or `-- <box-name>` | Nothing more |
| `takeover` (hold) | Leave control with a person and exit, then take it back later. | `cargo run --example takeover -- <box-name> hold`, then `-- <box-name> release` | A running box |
| `e2b_takeover` | A takeover on an E2B sandbox, with a public control viewer. | `cargo run --features e2b --example e2b_takeover -- <template-id> [query]` | `E2B_API_KEY` and an E2B template ID |

## Runtimes and images

| Example | Shows | Run | Needs |
| --- | --- | --- | --- |
| `custom_image` | Build an image on top of the bundled image, from `examples/images/acme/Dockerfile`, and launch a box on it. | `cargo run --example custom_image` | Docker |
| `custom_sandbox` | A remote vendor of your own, written as a `RemoteApi` implementation. This one runs its sandboxes on the local Docker. | `cargo run --example custom_sandbox` | Docker |
| `microvm` | Build the image, give it to the microsandbox hypervisor, and boot a box in a microVM. | `cargo run --example microvm` | Docker and microsandbox (`msb`) |
| `e2b` | Launch a box on E2B, drive it, and remove it. `--keep` leaves it running. | `cargo run --features e2b --example e2b -- <template-id> [--keep]` | `E2B_API_KEY` and an E2B template ID |
| `vercel` | Launch a box on Vercel, drive it, and remove it. With no image, it builds the built-in image at Vercel. It also takes a VCR image or an image directory. `--keep` leaves it running. | `cargo run --features vercel --example vercel [-- <image-or-dir>] [--keep]` | `VERCEL_TOKEN`, `VERCEL_TEAM_ID`, and `VERCEL_PROJECT_ID` |
| `daytona` | Launch a box on Daytona, drive it, and remove it. With no image, Daytona builds the built-in image. It also takes a snapshot name or an image directory. `--keep` leaves it running. | `cargo run --features daytona --example daytona [-- <snapshot-or-dir>] [--keep]` | `DAYTONA_API_KEY` |
| `modal` | Launch a box on Modal, drive it, and remove it. With no image, Modal builds the built-in image. It also takes a Modal image ID, a registry tag, or an image directory. `--keep` leaves it running. | `cargo run --features modal --example modal [-- <image-or-dir>] [--keep]` | `MODAL_TOKEN_ID` and `MODAL_TOKEN_SECRET` |

`examples/images/acme/Dockerfile` adds `htop`, `jq`, and a file to the bundled image. It keeps the `computer.profile` label, which a custom image needs.

## Full demos

The demos are separate Cargo projects. They are not in the workspace.

| Demo | Shows | Run | Needs |
| --- | --- | --- | --- |
| [`demos/jev`](../demos/jev/README.md) | A browser agent with a numbered action space. A choice model picks an operation and an element number from a snapshot. It includes a web inspector and examples for flights, native GTK forms, and a text editor. | `cd demos/jev && cargo run`, then open `http://127.0.0.1:8766` | `TYPESAFE_API_KEY`. `TEXT_MODEL_API_KEY` is optional. Copy `.env.example` to `.env` and set them. |
| [`demos/sketch`](../demos/sketch/README.md) | Turn a picture into pencil strokes, as a batch of `path` actions, and draw them in GIMP. | `python3 demos/sketch/sketch.py <picture> --box "$BOX" --find-canvas --draw` | Python with Pillow, and a box with `--app gimp` |

`demos/jev` also has examples that need no keys: `cargo run --example guards` drives a fixture page with a scripted policy. See its README for all its examples.
