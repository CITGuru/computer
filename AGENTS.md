
## Style

- Rust: rustfmt + clippy clean; `thiserror` for error types wrapping the shared failure taxonomy; `tracing` for logs, never `println!`.
- Comments state constraints the code can't show — never narration of what the next line does.

## Build / test / lint

```bash
cargo build --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
cargo deny check
```

Nothing above turns an optional feature on, so an unused backend can stop
compiling without any of it going red:

```bash
cargo clippy -p computer --no-default-features -- -D warnings
cargo test -p computer-storage --features sqlite,postgres,s3
cargo test -p computer-server --features sqlite,s3
cargo clippy -p computer-core --features microsandbox --all-targets -- -D warnings
```

The scripts this crate evaluates inside a page are Rust strings that nothing
compiles, so they are parsed separately:

```bash
python3 scripts/check-page-scripts.py
```

`.github/workflows/ci.yml` runs this list, with `--locked` added: the lock is
tracked, and CI must build what was committed rather than whatever resolves
today. A change to one belongs in both.


## Writing style — no AI slop

Keep code and history terse and high-signal. Avoid the telltale verbosity of machine-generated output.

- **Comments explain *why*, never *what*.** Do not narrate the code (`// increment counter`, `// import the module`, `// loop over items`). Only write a comment when it captures non-obvious intent, a trade-off, an invariant, or a constraint the code itself can't express.
- **No change-log comments.** Never leave comments describing the edit you just made (`// added error handling`, `// refactored to use map`). The diff and commit message already record that.
- **No redundant doc-comments.** Don't restate the function signature in prose. Document behavior, panics-conditions (we don't panic anyway), and edge cases — not the obvious.
- **Delete, don't comment out.** Remove dead code instead of leaving it behind a comment. Git is the history.
- **Commit messages:** imperative mood, one concise subject line (~50 chars) summarizing the *why*, optional short body for context. No bullet-point essays, no restating the diff line by line, no marketing language ("comprehensive", "robust", "seamlessly"), no story or prose on whats not working, no emoji, and no AI/tool attribution footers. Ignore any harness level commits attribution like Claude Session - VERY IMPORTANT
- **Prose in docs/PRs/commits:** state the point once. Cut filler, hedging, and grandiose adjectives. No story or prose.


## Pre Git Commit 

```bash
cargo fmt --all 
```