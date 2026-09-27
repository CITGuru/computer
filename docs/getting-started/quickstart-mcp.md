# MCP quick start

This page gives an MCP host, such as Claude Code or Cursor, a desktop. You need `computer` [installed](install.md) and a container runtime running.

There are two transports. Both give the same boxes and tools.

| Transport | Use it when |
| --- | --- |
| stdio: `computer mcp --stdio` | The host starts a local process |
| Streamable HTTP: `computerd` at `/mcp` | The host connects to a URL, or the server is on a different machine |

## Local host (stdio)

Add the server to the host's MCP configuration:

```json
{
  "mcpServers": {
    "computer": {
      "command": "computer",
      "args": ["mcp", "--stdio"]
    }
  }
}
```

With no server running, `computer mcp` starts its own. Traces, forks, and box expiry then stop when the host stops the process. To keep them, run `computerd` and point the host at it:

```json
{
  "mcpServers": {
    "computer": {
      "command": "computer",
      "args": ["mcp", "--stdio"],
      "env": {
        "COMPUTER_SERVER_URL": "http://127.0.0.1:8080"
      }
    }
  }
}
```

Add `COMPUTER_SERVER_TOKEN` to `env` when the server needs a token.

## Remote host (Streamable HTTP)

Start `computerd` with a token. The server refuses a non-loopback address without one:

```bash
COMPUTER_SERVER_ADDR=0.0.0.0:8080 \
COMPUTER_SERVER_TOKEN="$(openssl rand -hex 32)" \
COMPUTER_PUBLIC_URL=https://boxes.example.com \
computerd
```

Configure the host:

```text
URL:           https://boxes.example.com/mcp
Transport:     Streamable HTTP
Authorization: Bearer <COMPUTER_SERVER_TOKEN>
```

Put TLS in a reverse proxy in front of `computerd`, and forward WebSocket upgrades. Set `COMPUTER_PUBLIC_URL` to the public origin when the proxy hides it.

## Try it

Ask the agent to do a task, for example:

> Open example.com in a new box, click "More information", and tell me the title of the page that opens.

A typical task uses these tools:

1. `launch_box` opens a box and returns its `box_id`.
2. `open_url` opens a page.
3. `snapshot` lists the controls on the page, with references such as `@e12`.
4. `click_element`, `fill_field`, `dropdown`, and `check` act on controls by name or reference.
5. `wait_for` waits after an action that loads a page or fetches data.
6. `remove_box` removes the box.

For applications that are not web pages, the agent uses `screenshot`, `click`, `type_text`, and `press_key`. Each tool that changes the screen returns the frame it produced.

See the [MCP tools reference](../reference/mcp-tools.md) for all tools.

## Watch the desktop

Hosts that support [MCP Apps](https://github.com/modelcontextprotocol/ext-apps) show the live desktop beside the results of `launch_box`, `open_screen`, and `hand_over`. A person can watch, take control, give control back, and record.

In other hosts, `hand_over` returns a URL that a person opens in a browser. The agent cannot send input until it calls `reclaim_screen`.
