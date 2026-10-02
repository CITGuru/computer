# Deploy holmd

This guide runs `holmd` on a Linux host as a service that other machines, agents, and MCP hosts can reach. It uses Docker, systemd, and a reverse proxy for TLS.

For the concepts, see [The server](../concepts/server.md) and [Runtimes](../concepts/runtimes.md).

## 1. Prepare the host

You need:

- A Linux host with Docker, or another [runtime](../concepts/runtimes.md).
- A DNS name for the server, such as `boxes.example.com`.
- A reverse proxy that terminates TLS. This guide uses Caddy.

Plan the size of the host from the number of boxes that run at the same time. Each screen runs a display server, a window manager, Chromium, and a viewer. Measure one box on your host with `docker stats` before you decide.

## 2. Install

```bash
curl -fsSL https://raw.githubusercontent.com/CITGuru/computer/main/scripts/install.sh | HOLM_INSTALL_DIR=/usr/local/bin sh
```

Make a user for the service, and give it access to Docker:

```bash
sudo useradd --system --create-home --home-dir /var/lib/holm holm
sudo usermod -aG docker holm
```

Access to the Docker socket is equal to root access on the host. Run `holmd` on a host that is only for boxes.

## 3. Make the secrets

```bash
sudo install -d -m 0750 -o holm -g holm /etc/holm
openssl rand -hex 32 | sudo tee /etc/holm/token >/dev/null
sudo chmod 0600 /etc/holm/token
sudo chown holm:holm /etc/holm/token
```

The server token protects the API. Give it only to clients that can create and drive boxes.

If you will add cloud vendors with `holm runtime add`, the server also needs a key to encrypt their API keys. `HOLM_SERVER_SECRET_FILE` makes one at first start (step 4).

## 4. Configure

Write `/etc/holm/holmd.env`:

```bash
HOLM_SERVER_ADDR=127.0.0.1:8080
HOLM_PUBLIC_URL=https://boxes.example.com

HOLM_STORAGE_BACKEND=local
HOLM_STATE_DIR=/var/lib/holm/state

HOLM_SERVER_SECRET_FILE=/var/lib/holm/secret.key
HOLM_SERVER_CONFIG=/etc/holm/runtimes.toml

RUST_LOG=holmd=info,holm=info
```

- `HOLM_SERVER_ADDR` stays on loopback, because only the proxy connects to it.
- `local` storage keeps traces and remote runtimes across restarts. For more than one server, use `postgres` or `s3`. See [Storage](../concepts/server.md#storage).
- `RUST_LOG` sets the log level.

The token goes into the environment in step 5, from its file.

Write `/etc/holm/runtimes.toml`:

```toml
default = "docker"

[runtimes.docker]
memory = "4g"
lifetime = "1h"
max_lifetime = "8h"
```

Each box gets a lifetime, so a box that nobody removes does not run forever. See [The runtimes file](../concepts/runtimes.md#the-runtimes-file) for all fields.

## 5. Run as a systemd service

Write `/etc/systemd/system/holmd.service`:

```ini
[Unit]
Description=holmd
After=network-online.target docker.service
Wants=network-online.target
Requires=docker.service

[Service]
User=holm
Group=holm
EnvironmentFile=/etc/holm/holmd.env
ExecStart=/bin/sh -c 'HOLM_SERVER_TOKEN="$(cat /etc/holm/token)" exec /usr/local/bin/holmd'
KillSignal=SIGINT
TimeoutStopSec=30
Restart=on-failure
WorkingDirectory=/var/lib/holm

[Install]
WantedBy=multi-user.target
```

`KillSignal=SIGINT` is necessary. `holmd` shuts down cleanly and writes its store only on SIGINT. With the default SIGTERM, the last writes can be lost.

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now holmd
journalctl -u holmd -f
```

The log shows `holmd is listening` with the runtimes it found.

## 6. Put a proxy in front

The proxy must forward `/v1`, `/mcp`, and WebSocket upgrades. The viewers and the MCP Apps live screen use WebSockets.

Caddy forwards WebSockets with no extra settings:

```text
boxes.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

For nginx, add the upgrade headers:

```nginx
location / {
    proxy_pass http://127.0.0.1:8080;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_set_header Host $host;
    proxy_read_timeout 1h;
}
```

A long read timeout keeps viewer connections open.

## 7. Check it

```bash
curl https://boxes.example.com/v1/health

export HOLM_SERVER_URL=https://boxes.example.com
export HOLM_SERVER_TOKEN="$(sudo cat /etc/holm/token)"
BOX=$(holm new --ttl 10)
holm screenshot "$BOX" check.png
holm rm "$BOX"
```

The first box builds the image, which takes a few minutes.

## 8. Build images in advance

Each set of applications and packages is a different image. Build the images that your boxes use before the first request needs them:

```bash
holm image build docker
holm image build docker --app vscode --package jq
holm image ls
```

## 9. Connect clients

**CLI:** set `HOLM_SERVER_URL` and `HOLM_SERVER_TOKEN`, as in step 7.

**MCP host with a URL:**

```text
URL:           https://boxes.example.com/mcp
Transport:     Streamable HTTP
Authorization: Bearer <token>
```

**MCP host that starts a local process:**

```json
{
  "mcpServers": {
    "holm": {
      "command": "holm",
      "args": ["mcp", "--stdio"],
      "env": {
        "HOLM_SERVER_URL": "https://boxes.example.com",
        "HOLM_SERVER_TOKEN": "<token>"
      }
    }
  }
}
```

**Rust:** use [`holm-client`](../../crates/holm-client).

## 10. Let people see the boxes

A box publishes its own viewers only on the host's loopback address. From another machine:

- **In an MCP Apps host,** the live screen connects through `holmd`, so it works with no more setup. `HOLM_PUBLIC_URL` must be correct.
- **For viewer and takeover URLs,** the box must publish its viewers with a credential. Set `policy` in the box spec:

```json
{
  "spec": {
    "policy": { "auth": "token", "bind": "any", "advertise": "boxes.example.com" }
  }
}
```

The host firewall must then allow the box's viewer ports, and these URLs do not use the proxy's TLS. See [Viewer access](../concepts/human-control.md#viewer-access).

## Add a cloud vendor

```bash
printf '%s' "$E2B_API_KEY" | holm runtime add cloud --provider e2b --api-key
holm runtime ls
```

For Vercel, give the project and team as fields. See [Vercel Sandbox](vercel-sandbox.md).

```bash
holm runtime add vercel --provider vercel --field project_id=prj_... --field team_id=team_... --api-key-env VERCEL_TOKEN
```

The server encrypts the key with the key file from step 4. Keep that file: if it is lost, the server cannot decrypt the vendor keys, and the runtime shows `unavailable` until you set the key again.

## Upgrade

```bash
curl -fsSL https://raw.githubusercontent.com/CITGuru/computer/main/scripts/install.sh | HOLM_INSTALL_DIR=/usr/local/bin sh
sudo systemctl restart holmd
```

Boxes continue to run while the server restarts. At start, the server takes back each box in its records and each box that an earlier server labeled. With `local` storage or a database, their traces continue.

A new release can change the image source. Boxes started after the upgrade use a new image, so build the images again (step 8).

## Checklist

- [ ] `HOLM_SERVER_ADDR` is on loopback, and only the proxy is public.
- [ ] The server token is long and random, and only trusted clients have it.
- [ ] The service uses `KillSignal=SIGINT`.
- [ ] Storage is durable (`local`, `sqlite`, `postgres`, or `s3`).
- [ ] Each runtime has a `lifetime` and a `max_lifetime`.
- [ ] The secret key file has a backup, if you store vendor keys.
- [ ] The proxy forwards WebSocket upgrades.
- [ ] The host is only for boxes, because the service user can use Docker.
