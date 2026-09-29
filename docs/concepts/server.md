# The server

`computerd` is the server that owns boxes. It serves the REST API under `/v1`, MCP over Streamable HTTP at `/mcp`, and the viewers. The CLI and the stdio MCP server are clients of it.

The Rust library does not need a server. A `Computer` drives its box directly.

## Two binaries

| Binary | Role |
| --- | --- |
| `computer` | CLI, and MCP over stdio (`computer mcp --stdio`). It sends each command to a server. |
| `computerd` | The long-running server. |

## Temporary and long-running servers

When `computer` finds no server, it starts one inside its own process. That server stops when the command exits. You do not need to start anything for a quick test.

A temporary server cannot keep state between commands. Start `computerd` when you need:

- Traces, for `trace` and `fork`
- Box expiry, which removes boxes after `--ttl` or `--idle`
- Remote vendors added with `computer runtime add`
- Access from other hosts
- MCP over HTTP

The boxes do not depend on the server. A box that a temporary server made continues to run after the command exits. A later `computerd` finds it and takes it back.

## How a client finds a server

`computer` and `computer mcp` use the first of these:

1. `--server URL`
2. `COMPUTER_SERVER_URL`
3. A `computerd` on `http://127.0.0.1:8080`
4. A temporary server in the same process

The client checks that the service on port 8080 is `computerd`. It does not use an unrelated process on that port.

`--local` uses no server. It drives the local runtime directly and supports only a small set of commands. See the [CLI reference](../reference/cli.md#--local).

## Start the server

```bash
computerd
curl http://127.0.0.1:8080/v1/health
```

The server listens on `127.0.0.1:8080`. Set `COMPUTER_SERVER_ADDR` to change it.

At start, the server:

1. Finds the host runtimes and reads the [runtimes file](runtimes.md#the-runtimes-file).
2. Loads remote runtimes from the environment, the file, and the store.
3. Takes back each box in its records, then scans each runtime for boxes that an earlier server labeled.

## Access control

Anyone who can reach the API can create boxes, drive them, read their screens, and run commands in them. The server protects it with a bearer token.

| Address | Token |
| --- | --- |
| Loopback, such as `127.0.0.1` | Optional |
| Any other address | Necessary. The server does not start without one. |

```bash
COMPUTER_SERVER_ADDR=0.0.0.0:8080 \
COMPUTER_SERVER_TOKEN="$(openssl rand -hex 32)" \
computerd
```

Clients send `Authorization: Bearer <token>`. The CLI and `computer mcp` read the token from `COMPUTER_SERVER_TOKEN`.

- The token must be 16 characters or more.
- One token protects both `/v1` and `/mcp`.
- `/v1/health` needs no token, so a load balancer can use it.
- Viewer and CDP URLs have their own short-lived tokens. They do not contain the server token.

### Behind a reverse proxy

Put TLS in a reverse proxy in front of `computerd`. Forward `/v1`, `/mcp`, and WebSocket upgrades. The viewers use WebSockets.

Set `COMPUTER_PUBLIC_URL` to the public origin, such as `https://boxes.example.com`, when the proxy hides it. The MCP Apps live screen uses it to connect back to the server.

## Storage

The server stores box records, traces, frames, and remote runtimes. The default store is in memory, so all of this is lost when the server stops. The boxes themselves continue to run.

| `COMPUTER_STORAGE_BACKEND` | Settings | Build feature |
| --- | --- | --- |
| `memory` (default) | None | None |
| `local` | `COMPUTER_STATE_DIR` | None |
| `sqlite` | `COMPUTER_STATE_URL` | `sqlite` |
| `postgres` | `COMPUTER_STATE_URL` | `postgres` |
| `s3` | `COMPUTER_S3_ENDPOINT`, `COMPUTER_S3_BUCKET`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`; optional `COMPUTER_S3_REGION`, `COMPUTER_S3_PREFIX` | `s3` |

Release builds include the `sqlite`, `postgres`, and `s3` features.

```bash
COMPUTER_STORAGE_BACKEND=local \
COMPUTER_STATE_DIR=/var/lib/computer \
computerd
```

With the memory store:

- After a restart, the server takes the boxes back, but their traces start again, marked `adopted`. A fork can repeat only the actions after the restart.
- Remote runtimes added with `computer runtime add` are lost.

### Retention

| Variable | Default | Effect |
| --- | --- | --- |
| `COMPUTER_KEEP_FRAMES_SECS` | 2 hours | How long to keep frames |
| `COMPUTER_KEEP_ENTRIES_SECS` | 7 days | How long to keep trace entries |
| `COMPUTER_PRUNE_SECS` | 1 hour | How often to remove old data |

## Idempotency

Box creation, action batches, and forks accept an `idempotency-key` header. If a client sends the same request again with the same key, for example after a network error, the server returns the first result. It does not click two times or make a second box. The server keeps these keys in memory, so a restart forgets them.

## Errors

All API errors have the same shape:

```json
{ "code": "…", "message": "…", "retryable": false }
```

`retryable` tells a client if the same request can succeed later.

## Environment variables

| Variable | Default | Effect |
| --- | --- | --- |
| `COMPUTER_SERVER_ADDR` | `127.0.0.1:8080` | The listen address |
| `COMPUTER_SERVER_TOKEN` | None | The bearer token. Necessary on a non-loopback address. |
| `COMPUTER_PUBLIC_URL` | None | The public origin behind a reverse proxy |
| `COMPUTER_SERVER_CONFIG` | None | The path to the [runtimes file](runtimes.md#the-runtimes-file) |
| `COMPUTER_SERVER_RUNTIMES` | All that answer | The host runtimes to offer, separated by commas |
| `COMPUTER_SERVER_SANDBOXES` | None | Remote vendors to add from the environment, such as `e2b`, `vercel`, or `daytona` |
| `COMPUTER_SERVER_SECRET_KEY` | None | 32 bytes, base64 or hex, that encrypt stored vendor keys. With no key, `computer runtime add` is refused. |
| `COMPUTER_SERVER_SECRET_FILE` | None | A file that holds the secret key. If the file does not exist, the server makes it with mode `0600`. |
| `COMPUTER_SERVER_REAP_SECS` | 30 | How often to remove expired boxes |
| `COMPUTER_CONTENT_BOUNDARIES` | Off | `1` puts page text between nonce markers in MCP results |

Storage variables are in [Storage](#storage).
