# Contributing

This page tells you how the repository is organized, and which checks a change must pass.

## Requirements

- Rust 1.85 or newer, with `rustfmt` and `clippy`
- A container runtime, such as Docker, for the tests that start real boxes
- Node.js, only to build the MCP Apps page
- Python 3, for the page-script check
- `cargo-deny`, for the dependency check

## Workspace

| Crate | Role |
| --- | --- |
| `computer` (root) | The root package. It re-exports `computer-core`, and builds the `computer` and `computerd` binaries with the `cli` feature. |
| `computer-core` | The desktop and the API that drives it: `Computer`, screens, the browser, runtimes, images, and profiles. |
| `computer-types` | The types that describe a box and how to drive it, such as `Spec`, `Placement`, and `Point`. |
| `computer-api` | The HTTP contract of the server: request and response types. |
| `computer-client` | A Rust client for the server API. |
| `computer-server` | The REST server that manages and drives boxes. |
| `computer-storage` | Where the server keeps box records, traces, and frames: memory, local files, SQLite, PostgreSQL, or S3. |
| `computer-mcp` | The MCP server, and the MCP Apps page in `ui/`. |
| `computer-cli` | The `computer` command and the `computerd` daemon. |

`demos/` is not in the workspace. Each demo is its own Cargo project.

## Checks

Run these before you open a pull request:

```bash
cargo build --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
cargo deny check
```

The commands above do not turn on optional features, so a backend that nothing uses can stop compiling. Check the features too:

```bash
cargo clippy -p computer --no-default-features -- -D warnings
cargo test -p computer-storage --features sqlite,postgres,s3
cargo test -p computer-server --features sqlite,s3
cargo clippy -p computer-core --features microsandbox --all-targets -- -D warnings
```

The scripts that the crate runs inside a page are Rust strings, so the compiler does not check them. Parse them:

```bash
python3 scripts/check-page-scripts.py
```

The shell scripts that go into an image are not compiled either. Check their syntax:

```bash
for script in crates/computer-core/images/*/*.sh; do bash -n "$script"; done
```

The MCP Apps page is built and committed. A change under `crates/computer-mcp/ui/src` has no effect until you build it. Build it, and check that the committed page matches:

```bash
crates/computer-mcp/ui/build.sh
git diff --exit-code -- crates/computer-mcp/ui/screen.html
```

Commit the built `screen.html` with the source change.

### Continuous integration

`.github/workflows/ci.yml` runs the same checks with `--locked`. The lock file is committed, and CI builds what was committed. When you change a check, change it in `AGENTS.md` and in `ci.yml`.

CI runs the build and tests on Ubuntu and macOS. It also checks `scripts/install.sh` with `sh -n` and `shellcheck`, and `scripts/release.sh` with `bash -n`.

### Tests that start real boxes

Tests that need a container runtime, an E2B account, or a hypervisor are marked `#[ignore]`. Run them on purpose:

```bash
cargo test --test live -- --ignored
```

The files in `tests/` that start with `live_` are this kind of test.

## Images

The desktop images are in `crates/computer-core/images/`:

| Directory | Image |
| --- | --- |
| `desktop` | The default X11 image |
| `wayland` | The Wayland image |
| `tiny` | The default image with noVNC copied from a build stage, so the image does not carry its build tools |
| `ubuntu` | The same desktop on Ubuntu 24.04 |

The image tag contains a hash of the image source. A change to a file in these directories makes a new tag, so the next box builds a new image. It never reuses an image built from old source.

## Code style

- Format with `rustfmt`. Clippy must pass with `-D warnings`.
- Use `thiserror` for error types.
- Use `tracing` for logs. Do not use `println!` in library code.
- A comment tells why: an intent, a trade-off, an invariant, or a limit that the code cannot show. Do not write comments that tell what the next line does.
- Do not leave code commented out. Delete it.
- Doc comments tell behavior and edge cases. They do not repeat the signature.

## Commit messages

- Use the imperative mood.
- Write one short subject line, about 50 characters, that says what the change does.
- Add a short body only when it gives context that the diff does not.
- Do not add lists that repeat the diff, marketing words, emoji, or attribution lines.

Examples: `Build the E2B template this crate needs`, `Record every image this server builds`.
