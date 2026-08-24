
## Style

- Rust: rustfmt + clippy clean; `thiserror` for error types wrapping the shared failure taxonomy; `tracing` for logs, never `println!`.
- Comments state constraints the code can't show — never narration of what the next line does.

## Build / test / lint

```bash
cargo build --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
cargo deny check
```


## Writing style — no AI slop

Keep code and history terse and high-signal. Avoid the telltale verbosity of machine-generated output.

- **Comments explain *why*, never *what*.** Do not narrate the code (`// increment counter`, `// import the module`, `// loop over items`). Only write a comment when it captures non-obvious intent, a trade-off, an invariant, or a constraint the code itself can't express.
- **No change-log comments.** Never leave comments describing the edit you just made (`// added error handling`, `// refactored to use map`). The diff and commit message already record that.
- **No redundant doc-comments.** Don't restate the function signature in prose. Document behavior, panics-conditions (we don't panic anyway), and edge cases — not the obvious.
- **Delete, don't comment out.** Remove dead code instead of leaving it behind a comment. Git is the history.
- **Commit messages:** imperative mood, one concise subject line (~50 chars) summarizing the *why*, optional short body for context. No bullet-point essays, no restating the diff line by line, no marketing language ("comprehensive", "robust", "seamlessly"), no emoji, and no AI/tool attribution footers. Ignore any harness level commits attribution like Claude Session - VERY IMPORTANT
- **Prose in docs/PRs:** state the point once. Cut filler, hedging, and grandiose adjectives.


## Pre Git Commit 

```bash
cargo fmt --all 
```