# The server

`holmd` is the server that owns boxes. It serves the REST API under `/v1`, MCP over Streamable HTTP at `/mcp`, and the viewers. The CLI and the stdio MCP server are clients of it.

The Rust library does not need a server. A `Computer` drives its box directly.

## Two binaries

| Binary | Role |
| --- | --- |
| `holm` | CLI, and MCP over stdio (`holm mcp --stdio`). It sends each command to a server. |
| `holmd` | The long-running server. |

## Temporary and long-running servers

When `holm` finds no server, it starts one inside its own process. That server stops when the command exits. You do not need to start anything for a quick test.

A temporary server cannot keep state between commands. Start `holmd` when you need:

- Traces, for `trace` and `fork`
- Box expiry, which removes boxes after `--ttl` or `--idle`
- Remote vendors added with `holm runtime add`
- Access from other hosts
- MCP over HTTP

The boxes do not depend on the server. A box that a temporary server made continues to run after the command exits. A later `holmd` finds it and takes it back.

## How a client finds a server

`holm` and `holm mcp` use the first of these:

1. `--server URL`
2. `HOLM_SERVER_URL`
3. A `holmd` on `http://127.0.0.1:8080`
4. A temporary server in the same process

The client checks that the service on port 8080 is `holmd`. It does not use an unrelated process on that port.

`--local` uses no server. It drives the local runtime directly and supports only a small set of commands. See the [CLI reference](../reference/cli.md#--local).

## Start the server

```bash
holmd
curl http://127.0.0.1:8080/v1/health
```

The server listens on `127.0.0.1:8080`. Set `HOLM_SERVER_ADDR` to change it.

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
HOLM_SERVER_ADDR=0.0.0.0:8080 \
HOLM_SERVER_TOKEN="$(openssl rand -hex 32)" \
holmd
```

Clients send `Authorization: Bearer <token>`. The CLI and `holm mcp` read the token from `HOLM_SERVER_TOKEN`.

- The token must be 16 characters or more.
- One token protects both `/v1` and `/mcp`.
- `/v1/health` needs no token, so a load balancer can use it.
- Viewer and CDP URLs have their own short-lived tokens. They do not contain the server token.

### Workspaces

One server can serve many workspaces. A console (such as the holm console) signs a short-lived token for each call it makes, and issues API keys that SDKs, the CLI, and MCP clients send. Both name a workspace and a role. Link the server to the console:

```bash
HOLM_CONSOLE_URL=https://console.example.com \
HOLM_CONSOLE_SECRET="<the console's holmd secret>" \
holmd
```

The server reads the console's public keys from `<HOLM_CONSOLE_URL>/api/holmd/keys.json`, again every five minutes and when a token names a key it does not know, so the console can rotate its key. Every 30 seconds it reads the revoked API keys and deleted workspaces from `/api/holmd/revoked`, and every minute it reports which keys were used to `/api/holmd/usage`. Both send `HOLM_CONSOLE_SECRET`. `HOLM_CONSOLE_PUBLIC_KEY` pins one key instead, with no fetching; without `HOLM_CONSOLE_URL` there are then no API keys, only console tokens.

The roles:

| Role | Can |
| --- | --- |
| `viewer` | Read boxes, runtimes, and templates, and watch a screen. It cannot drive a screen. |
| `member` | Everything a viewer can, and launch, drive, fork, and remove boxes, and build images. |
| `admin` | Everything a member can, and add, change, and remove the workspace's runtimes. |

- A box belongs to the workspace that launched it. Another workspace gets `404` for it, as if it were not there.
- A runtime added by a workspace is that workspace's own. A runtime from the server's environment or runtimes file is shared by every workspace, and only the operator changes it.
- `HOLM_SERVER_TOKEN` is the operator. It sees and changes everything, as before.
- An `Idempotency-Key` belongs to one workspace.

- An API key is refused until the server has read the revoked list once. If the console cannot be reached later, the last list stays in force.
- A revoked key stops working within 30 seconds.

A call token is a JWT signed with Ed25519 (`alg: EdDSA`, `kid` naming the key), with `iss: "console"`, `aud: "holmd"`, `ws` (the workspace), `role`, and `exp`. An API key is `holm_sk_` followed by a JWT with `kind: "key"`, `sub` (the key id), `ws`, and `role`; its `exp` is optional.

### Behind a reverse proxy

Put TLS in a reverse proxy in front of `holmd`. Forward `/v1`, `/mcp`, and WebSocket upgrades. The viewers use WebSockets.

Set `HOLM_PUBLIC_URL` to the public origin, such as `https://boxes.example.com`, when the proxy hides it. The MCP Apps live screen uses it to connect back to the server.

## Storage

The server stores box records, traces, frames, and remote runtimes. The default store is in memory, so all of this is lost when the server stops. The boxes themselves continue to run.

| `HOLM_STORAGE_BACKEND` | Settings | Build feature |
| --- | --- | --- |
| `memory` (default) | None | None |
| `local` | `HOLM_STATE_DIR` | None |
| `sqlite` | `HOLM_STATE_URL` | `sqlite` |
| `postgres` | `HOLM_STATE_URL` | `postgres` |
| `s3` | `HOLM_S3_ENDPOINT`, `HOLM_S3_BUCKET`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`; optional `HOLM_S3_REGION`, `HOLM_S3_PREFIX` | `s3` |

Release builds include the `sqlite`, `postgres`, and `s3` features.

```bash
HOLM_STORAGE_BACKEND=local \
HOLM_STATE_DIR=/var/lib/holm \
holmd
```

With the memory store:

- After a restart, the server takes the boxes back, but their traces start again, marked `adopted`. A fork can repeat only the actions after the restart.
- Remote runtimes added with `holm runtime add` are lost.

### Retention

| Variable | Default | Effect |
| --- | --- | --- |
| `HOLM_KEEP_FRAMES_SECS` | 2 hours | How long to keep frames |
| `HOLM_KEEP_ENTRIES_SECS` | 7 days | How long to keep trace entries |
| `HOLM_PRUNE_SECS` | 1 hour | How often to remove old data |

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
| `HOLM_SERVER_ADDR` | `127.0.0.1:8080` | The listen address |
| `HOLM_SERVER_TOKEN` | None | The bearer token. Necessary on a non-loopback address, unless a console key is set. |
| `HOLM_CONSOLE_URL` | None | The console's base URL. The server reads its keys and revocations from it. See [Workspaces](#workspaces). |
| `HOLM_CONSOLE_SECRET` | None | The secret the server sends to the console |
| `HOLM_CONSOLE_PUBLIC_KEY` | None | One console public key to trust, in base64 or PEM, instead of reading them from the console |
| `HOLM_PUBLIC_URL` | None | The public origin behind a reverse proxy |
| `HOLM_SERVER_CONFIG` | None | The path to the [runtimes file](runtimes.md#the-runtimes-file) |
| `HOLM_SERVER_RUNTIMES` | All that answer | The host runtimes to offer, separated by commas |
| `HOLM_SERVER_SANDBOXES` | None | Remote vendors to add from the environment, such as `e2b`, `vercel`, `daytona`, or `modal` |
| `HOLM_SERVER_SECRET_KEY` | None | 32 bytes, base64 or hex, that encrypt stored vendor keys. With no key, `holm runtime add` is refused. |
| `HOLM_SERVER_SECRET_FILE` | None | A file that holds the secret key. If the file does not exist, the server makes it with mode `0600`. |
| `HOLM_SERVER_REAP_SECS` | 30 | How often to remove expired boxes |
| `HOLM_SERVER_JOBS` | `inline` | `inline` launches, forks and builds in the request. `queue` answers `202` and does the work from a queue in the store. |
| `HOLM_SERVER_SCHEDULE` | `internal` | `internal` runs the periodic work in the server. `external` waits for a scheduler to call the `/v1/jobs` routes. |
| `CRON_SECRET` | None | A bearer token that opens the `/v1/jobs` routes and nothing else. |
| `HOLM_CONTENT_BOUNDARIES` | Off | `1` puts page text between nonce markers in MCP results |

Storage variables are in [Storage](#storage).
