# Configuration reference

This page lists all settings in one place. The linked pages explain them.

## Client

`holm` and `holm mcp --stdio` read these.

| Variable | Default | Effect |
| --- | --- | --- |
| `HOLM_SERVER_URL` | None | The server to use. `--server` overrides it. See [How a client finds a server](../concepts/server.md#how-a-client-finds-a-server). |
| `HOLM_SERVER_TOKEN` | None | The bearer token for the server. |
| `HOLM_CONTENT_BOUNDARIES` | Off | `1` puts page text between nonce markers in `read`, `snapshot`, `find`, `eval`, and `console`. |

## Server

`holmd` reads these. See [The server](../concepts/server.md).

| Variable | Default | Effect |
| --- | --- | --- |
| `HOLM_SERVER_ADDR` | `127.0.0.1:8080` | The listen address. |
| `HOLM_SERVER_TOKEN` | None | The bearer token. Necessary on a non-loopback address, unless a console is linked. At least 16 characters. |
| `HOLM_CONSOLE_URL` | None | A console's base URL. The server accepts the tokens and API keys that console signs for a workspace. See [Workspaces](../concepts/server.md#workspaces). |
| `HOLM_CONSOLE_SECRET` | None | The secret the server sends when it reads revoked keys from the console and reports key use to it. |
| `HOLM_CONSOLE_PUBLIC_KEY` | None | One console public key to trust, in base64 or PEM, instead of reading the keys from the console. `HOLM_CONSOLE_PUBLIC_KEY_FILE` reads it from a file. |
| `HOLM_PUBLIC_URL` | None | The public origin behind a reverse proxy. |
| `HOLM_SERVER_REAP_SECS` | `30` | How often to remove expired boxes, in seconds. |
| `HOLM_SERVER_JOBS` | `inline` | `inline` launches, forks and builds in the request. `queue` puts the work in the store, answers `202`, and the server's workers do it. |
| `HOLM_SERVER_SCHEDULE` | `internal` | `internal` runs the periodic work and the job workers in the server. With more than one server on one database, one of them runs the periodic work. `external` runs nothing by itself: a scheduler calls the `/v1/jobs` routes. |
| `CRON_SECRET` | None | A bearer token that opens the `/v1/jobs` routes and nothing else. Vercel Cron sends it. |
| `HOLM_CONTENT_BOUNDARIES` | Off | `1` applies content boundaries to `/mcp`. |

### Runtimes

See [Runtimes](../concepts/runtimes.md).

| Variable | Default | Effect |
| --- | --- | --- |
| `HOLM_SERVER_CONFIG` | None | The path to the [runtimes file](#runtimes-file). Read at start. |
| `HOLM_SERVER_RUNTIMES` | All that answer | Host runtimes to offer, separated by commas: `docker`, `podman`, `nerdctl`, `smolvm`, `microsandbox`. |
| `HOLM_SERVER_SANDBOXES` | None | Remote vendors to add from the environment, separated by commas, such as `e2b`, `vercel`, `daytona`, or `modal`. |
| `HOLM_SERVER_SECRET_KEY` | None | 32 bytes, base64 or hex. Encrypts vendor keys that the server stores. |
| `HOLM_SERVER_SECRET_FILE` | None | A file that holds the secret key. The server makes it, with mode `0600`, if it does not exist. |

### Storage

See [Storage](../concepts/server.md#storage).

| Variable | Default | Effect |
| --- | --- | --- |
| `HOLM_STORAGE_BACKEND` | `memory` | `memory`, `local`, `sqlite`, `postgres`, or `s3`. |
| `HOLM_STATE_DIR` | None | The directory for `local`. |
| `HOLM_STATE_URL` | None | The database URL for `sqlite` and `postgres`. |
| `HOLM_STATE_POOL` | `10` | The most database connections one server process holds. Set it low when many processes share a pooler with a client limit. |
| `HOLM_S3_ENDPOINT` | None | The S3 endpoint for `s3`. |
| `HOLM_S3_BUCKET` | None | The bucket for `s3`. |
| `HOLM_S3_REGION` | None | The region for `s3`. |
| `HOLM_S3_PREFIX` | None | A key prefix for `s3`. |
| `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` | None | Credentials for `s3`. |
| `HOLM_KEEP_FRAMES_SECS` | 2 hours | How long to keep frames. |
| `HOLM_KEEP_ENTRIES_SECS` | 7 days | How long to keep trace entries. |
| `HOLM_PRUNE_SECS` | 1 hour | How often to remove old data. |

## E2B

The E2B client reads these, in the library and in the server.

| Variable | Default | Effect |
| --- | --- | --- |
| `E2B_API_KEY` | None | The API key. Necessary. |
| `E2B_DOMAIN` | `e2b.app` | The E2B domain, for a self-hosted E2B. |

## Vercel

The Vercel client reads these, in the library and in the server.

| Variable | Default | Effect |
| --- | --- | --- |
| `VERCEL_TOKEN` | None | The access token. Necessary. It must be able to write to Vercel Container Registry. |
| `VERCEL_TEAM_ID` | None | The team that owns the project. Necessary for the image build. |
| `VERCEL_PROJECT_ID` | None | The project for the sandboxes and the image. Necessary. |
| `VERCEL_TEAM_SLUG` | Read from the API | The team slug for the registry path, when the token cannot read the team. |

## Daytona

The Daytona client reads these, in the library and in the server.

| Variable | Default | Effect |
| --- | --- | --- |
| `DAYTONA_API_KEY` | None | The API key. Necessary. |
| `DAYTONA_API_URL` | `https://app.daytona.io/api` | The API URL. |
| `DAYTONA_TARGET` | Daytona's | The region, such as `us` or `eu`. |

## Modal

The Modal client reads these, in the library and in the server.

| Variable | Default | Effect |
| --- | --- | --- |
| `MODAL_TOKEN_ID` | None | The token ID. Necessary. |
| `MODAL_TOKEN_SECRET` | None | The token secret. Necessary. |
| `MODAL_ENVIRONMENT` | The workspace default | The Modal environment. |
| `MODAL_APP` | `holm` | The Modal app that holds the images and sandboxes. |
| `MODAL_SERVER_URL` | `https://api.modal.com:443` | The API address. |

## Container engines

The engines read their own settings. The server does not change them.

| Setting | Effect |
| --- | --- |
| `DOCKER_HOST`, `docker context` | Point Docker at a different host. |
| `default-runtime` in `daemon.json` | Put all Docker containers on another OCI runtime, such as gVisor (`runsc`) or Kata. |

## Runtimes file

A TOML file at `HOLM_SERVER_CONFIG`. See [The runtimes file](../concepts/runtimes.md#the-runtimes-file).

```toml
default = "docker"

[runtimes.docker]
memory = "4g"

[runtimes.hardened]
provider = "docker"
isolation = "runsc"

[runtimes.e2b]
lifetime = "4h"
max_lifetime = "24h"
```

Top level:

| Key | Effect |
| --- | --- |
| `default` | The runtime for a box that names none. Default `docker`. |
| `[runtimes.NAME]` | A table for each runtime. |

Runtime tables:

| Field | Runtimes | Effect |
| --- | --- | --- |
| `provider` | Host | With it, the table adds a new runtime on that engine. Without it, the table changes the runtime with the same name. |
| `enabled` | Host | `false` hides a runtime that the server found. |
| `memory` | All | Default memory for each box, such as `"4g"`. |
| `cpus` | All | Default CPUs for each box. |
| `lifetime` | All | Lifetime of a box that does not give one. Default one hour. |
| `max_lifetime` | All | The longest lifetime a box can ask for. |
| `isolation` | docker, podman, nerdctl | OCI runtime for each box, such as `runsc`. |
| `context` | docker | A Docker context name. |
| `program` | smolvm, microsandbox | Path to the hypervisor program. |
| `build_with` | smolvm, microsandbox | The engine that builds images. Default `docker`. |

Times are `24h`, `90m`, `3600s`, or a number of seconds. The server refuses unknown fields, and refuses a `lifetime` that is more than `max_lifetime`.

## Vendor fields

Fields for a remote runtime added with `holm runtime add --field name=value` or `POST /v1/runtimes`.

| Field | Effect |
| --- | --- |
| `endpoint` | The API endpoint of a self-hosted vendor. Through the API, it must be `https` and a public address. |
| `image` | A template ID to use for all boxes, instead of a template that the server builds. |
| `lifetime_secs` | Lifetime of a box that does not give one. |
| `max_lifetime_secs` | The longest lifetime a box can ask for. |
| `public_traffic` | `true` lets anyone reach the box's published ports with no E2B traffic token. Default `false`. |
| `project_id` | Vercel: the project. Necessary. |
| `team_id` | Vercel: the team that owns the project. |
| `team_slug` | Vercel: the team slug, when the token cannot read the team. |
| `target` | Daytona: the region. |
| `token_id` | Modal: the token ID. The secret goes in as `api_key`. |
| `environment` | Modal: the environment. |
| `app` | Modal: the app. |

Secrets, such as `api_key`, go in with `--api-key` or `--api-key-env`, or under `secrets` in the API. The server never returns them.

## Cargo features

For the root `holm` package. See [Install](../getting-started/install.md#build-from-source).

| Feature | Default | Effect |
| --- | --- | --- |
| `cli` | On | The `holm` and `holmd` binaries. Turn it off for a library-only dependency. |
| `e2b` | Off | The E2B client. |
| `vercel` | Off | The Vercel Sandbox client. |
| `daytona` | Off | The Daytona client. |
| `modal` | Off | The Modal client. |
| `microsandbox` | Off | The microsandbox Rust library. The server uses the `msb` CLI with no feature. |
| `sqlite` | Off | SQLite storage for the server. |
| `postgres` | Off | PostgreSQL storage for the server. |
| `s3` | Off | S3 storage for the server. |

Release builds turn on `e2b`, `vercel`, `daytona`, `modal`, `sqlite`, `postgres`, and `s3`.

## Box settings

Each box setting is available from each interface.

| Setting | CLI (`holm new`) | MCP (`launch_box`) | REST (`POST /v1/boxes`) | Rust builder |
| --- | --- | --- | --- | --- |
| Screen size | `--size WxH` | `width`, `height` | `spec.desktop.width`, `height` | `size(w, h)` |
| Screens | `--screens N` | `screens` | `spec.desktop.screens` | `Builder::from_spec` |
| Wayland | `--wayland` | `wayland` | `spec.desktop.server: "wayland"` | `profile(Arc::new(WaylandProfile))` |
| Applications | `--app NAME` | `apps` | `spec.apps` | `Builder::from_spec` |
| Packages | `--package PKG` | `packages` | `spec.desktop.packages` | `packages([…])` |
| Features (default: wide fonts, video, dock, accessibility, and X11 apps on Wayland) | `--base`, `--minimal`, `--audio` | `minimal`; `wide_fonts`, `audio`, `video`, `accessibility`: `true` adds, `false` leaves out | `spec.desktop.features` | `minimal()`, `audio()`, `without(feature)`, `features([…])` |
| Network | `--no-network` | `network: false` | `spec.policy.network` | `network(false)` |
| Runtime | `--runtime NAME` | `runtime` | `placement.runtime` | `runtime(name)`, `machine(…)` |
| Memory | `--memory SIZE` | `memory` | `placement.memory` | `memory(size)` |
| CPUs | `--cpus N` | `cpus` | `placement.cpus` | `cpus(n)` |
| Lifetime | `--ttl MINUTES` | `ttl_minutes` | `placement.expires_after_secs` | `expires_after(duration)` |
| Idle timeout | `--idle MINUTES` | `idle_minutes` | `placement.idle_timeout_secs` | `expires_when_idle(duration)` |
| Keep files when stopped | `--persistent` | `persistent` | `placement.persistent` | `persistent(true)` |
| Browser profile | `--profile NAME` | `profile` | `placement.profile` | `profiles(volume)` |
| Viewer access | `--spec FILE` | | `spec.policy.auth`, `bind`, `advertise` | `auth(…)`, `publish_on(…)`, `advertise(…)` |
| Full spec | `--spec FILE` | | The request body | |
