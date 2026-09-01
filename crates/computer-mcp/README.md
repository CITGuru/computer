# computer-mcp

An MCP server that gives an agent a desktop.

Speaks JSON-RPC over stdio to whatever launched it, and HTTP to a
`computer-server` — so the boxes can be on this machine or on a fleet somewhere
else, and nothing about the tools changes.

```bash
cargo run -p computer-server            # the boxes live here
cargo install --path crates/computer-mcp
```

```json
{
  "mcpServers": {
    "computer": {
      "command": "computer-mcp",
      "env": {
        "COMPUTER_SERVER_URL": "http://127.0.0.1:8080",
        "COMPUTER_SERVER_TOKEN": "…"
      }
    }
  }
}
```

## The tools

`launch_box` · `list_boxes` · `remove_box` · `screenshot` · `open_url` ·
`click` · `type_text` · `press_key` · `scroll` · `drag` · `run_command` ·
`hand_over` · `reclaim_screen` · `fork_box`

**Every tool that moves the screen answers with the frame it produced**, as an
image rather than a hash. An agent that has to ask for a screenshot after every
click spends two round trips on one step, and the second one is where it forgets
to look.

A tool that fails answers with `isError` and the reason, not a protocol error —
the model is the one that has to act on it, and a JSON-RPC error never reaches
it. Only a malformed call gets a protocol error.

`hand_over` returns a URL a person opens, and holds the agent's input back until
`reclaim_screen`. `fork_box` builds a second box by doing again what was done to
the first; it reconstructs rather than copies, so the two will be close and
rarely identical.

## Stdout carries the protocol

Logs go to stderr. A stray line on stdout is a parse error at the other end.
