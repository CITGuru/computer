# Vercel Sandbox

Vercel Sandbox runs each box in a Firecracker microVM at Vercel. You need no container runtime on your host, and no Docker to build the image: the server builds it at Vercel.

For how remote runtimes compare with host runtimes, see [Runtimes](../concepts/runtimes.md). For E2B, see [Cloud sandboxes](cloud-sandboxes.md).

## What works

| Works | Not yet supported |
| --- | --- |
| Screenshots, mouse, keyboard, and waits | Pause and stop |
| Windows, applications, and native widgets | Browser profiles (`--profile`) |
| Files, commands, and clipboard | Applications and packages added at launch (`--app`, extras) |
| Page tools and `computer cdp` | Page tools on boxes that a restarted server finds again |
| Viewer and takeover URLs in a browser | More than 6 screens |
| CPUs and memory for each box | |
| Network policy (`--no-network`) | |

## 1. Get a token and the IDs

You need three values from Vercel:

| Variable | Where to find it |
| --- | --- |
| `VERCEL_TOKEN` | Account settings, Tokens. Scope the token to the team. The token must be able to write to Vercel Container Registry. |
| `VERCEL_TEAM_ID` | Team settings. |
| `VERCEL_PROJECT_ID` | Project settings. The sandboxes and the image belong to this project. |

If the token cannot read the team, also set `VERCEL_TEAM_SLUG` to the team's slug: the first part of the dashboard URL, `vercel.com/<slug>`. The registry path needs it.

Keep the token out of shell history and out of files that go into git.

Release builds of `computer` and `computerd` include Vercel. For a source build, add the `vercel` feature:

```bash
cargo install --path . --locked --features vercel
```

## 2. Add Vercel to the server

There are two ways. Use one.

**From the environment.** The runtime is named `vercel`:

```bash
VERCEL_TOKEN=... VERCEL_TEAM_ID=... VERCEL_PROJECT_ID=... \
COMPUTER_SERVER_SANDBOXES=vercel computerd
```

**At run time, with the CLI.** You choose the name. The server keeps the runtime in its store:

```bash
computer runtime add cloud --provider vercel \
  --field project_id=prj_... \
  --field team_id=team_... \
  --api-key-env VERCEL_TOKEN
computer runtime ls
```

The token goes in as the runtime's `api_key`. To keep it, the server needs `COMPUTER_SERVER_SECRET_KEY` or `COMPUTER_SERVER_SECRET_FILE`. Add `--field team_slug=...` when the token cannot read the team.

## 3. Create a box

```bash
BOX=$(computer new --runtime vercel)
```

MCP: `launch_box` with `runtime: "vercel"`. REST: `"placement": {"runtime": "vercel"}`.

The first box waits for the image build, about two minutes. Later boxes start in about 10 seconds.

## Images

Vercel starts a sandbox only from an image in Vercel Container Registry (VCR), for `linux/amd64`. It does not run the image's `ENTRYPOINT` or `CMD`, so the server starts the desktop with a command after the sandbox starts.

### The built-in image

When a box names no image, the server does this:

1. It tags the built-in image `computer-desktop:<fingerprint>-x86_64`. The fingerprint comes from the image files, so a changed image gets a new tag.
2. It asks the registry for that tag. If the tag is there, the box starts from it.
3. If the tag is not there, it starts a builder sandbox with 4 vCPUs, uploads the image files, installs Docker, builds for `linux/amd64`, and pushes to `vcr.vercel.com/<team-slug>/<project>/computer-desktop`. Then it removes the builder sandbox.

The builder sandbox gets the token for the push. It lives at most 30 minutes, and the server removes it when the build ends, also when the build fails.

### An image directory

An image directory (`image_dir` in the library) is built the same way. All files in the directory go to the builder, with their modes. The image is `computer-local:<fingerprint>-x86_64`. A symlink in the directory is refused.

### An image that you name

A named image goes to Vercel as it is, with no build. It must:

- Be in VCR: `my-repo:tag` in the runtime's project, `team/project/repo:tag` for a shared or public repository, or `vcr.vercel.com/team/project/repo:tag`.
- Be built for `linux/amd64`.
- Contain the desktop: build it from the built-in Dockerfile, or on top of an image built from it.

An image that is not in VCR is refused with "not in Vercel Container Registry for this project". Images from other registries, such as Docker Hub, are not in VCR.

To push an image yourself:

```bash
docker buildx build --platform linux/amd64 \
  -t vcr.vercel.com/<team-slug>/<project>/computer-desktop:mine --push \
  crates/computer-core/images/desktop
```

`docker login vcr.vercel.com` fails, because the registry answers 404 to Docker's first check. Put the login in Docker's credential store instead. The user name is the team ID, and the password is the token.

## CPUs and memory

Vercel gives 2 GB of memory for each vCPU, and takes a vCPU count of 1 or an even number. The server takes the larger of `--cpus` and `--memory` divided by 2 GB, and rounds an odd count above 1 up. For example, `--memory 6g` gives 4 vCPUs and 8 GB. The most vCPUs depends on your plan.

With neither, Vercel's default applies.

## Ports and screens

Vercel publishes at most 14 ports for each sandbox. Each screen needs two ports, and page tools need one. So a box has at most 6 screens on Vercel.

## Viewers and page tools

Vercel publishes each port at a public URL, with no gate of its own. The server always sets token access on a remote box with no other viewer access, so the viewer and takeover URLs need their own token.

Page tools reach Chromium through a bridge in the box on port 9223. The bridge refuses each request without the secret that the server made for the box, so the public address alone does not open the browser. The secret stays with the server that created the box. After a restart, the server finds the box again, but page tools answer 403 on it.

## Lifetimes

Vercel limits how long a sandbox can live: 45 minutes on Hobby, and 24 hours on Pro and Enterprise. The server does not know your plan, so set the limit on the runtime:

```toml
[runtimes.vercel]
lifetime = "30m"
max_lifetime = "45m"
```

For a runtime added with the CLI, use `--field lifetime_secs=1800 --field max_lifetime_secs=2700`.

While a box is in use, the server extends its deadline at Vercel.

## Restart

Vercel allows at most 5 tags for each sandbox, with values of at most 256 characters. The server's record of a box is longer. So the box name goes in a tag, and all labels go in `/tmp/computer-labels.json` in the box. When `computerd` restarts, it reads that file to take the box back.

Each sandbox is created as not persistent. A stop removes it, and Vercel keeps no snapshot.

## Use Vercel from Rust

The library uses Vercel with no server. Build with the `vercel` feature, and set the variables from step 1.

```rust
use computer::sandboxes::{remote, vercel::cloud::Cloud};
use computer::{Auth, Computer, X11Profile};

let (machine, profile) = remote::pair(Arc::new(Cloud::from_env()?), Arc::new(X11Profile));

let computer = Computer::builder()
    .machine(Arc::new(
        machine
            .public_viewer(true)
            .expiring_after(Duration::from_secs(15 * 60)),
    ))
    .profile(profile)
    .auth(Auth::Token)
    .launch()
    .await?;
```

| Setting | Effect |
| --- | --- |
| No `image` | Build the built-in image at Vercel when the registry does not have it, then start from it. |
| `.image("<repo>:<tag>")` | Start from this VCR image. No build. |
| `.image_dir(path)` | Build this directory at Vercel, then start from it. |
| `public_viewer(true)` | Give the box a viewer URL. The launch needs `Auth::Token` or `Auth::Password`. |
| `expiring_after(duration)` | How long the sandbox lives with no activity. Activity extends it. |
| `Cloud::team_slug(slug)` | The team slug, when the token cannot read it. |

Run the example. With no argument, it builds the built-in image. It also takes a VCR image or an image directory:

```bash
cargo run --features vercel --example vercel
cargo run --features vercel --example vercel -- computer-desktop:mine
cargo run --features vercel --example vercel -- path/to/image-dir --keep
```
