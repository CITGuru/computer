# Custom images

By default, a box uses an image that the crate builds from its own Dockerfile. This guide uses a different image: one built from your own Dockerfile, or one that is already in a registry.

Custom images are a feature of the Rust library. The CLI, REST API, and MCP tools use the built-in image, with the applications and packages that a [spec](configure-a-box.md) adds.

## Select an approach

| You want to | Do this |
| --- | --- |
| Add packages or a catalog application | Use `packages([…])` or a spec. You do not need a custom image. See [Configure a box](configure-a-box.md). |
| Add files, configuration, or tools that apt does not have | Build an image `FROM` the built-in image. See [Extend the built-in image](#extend-the-built-in-image). |
| Start boxes with no build step | Build the image in CI, push it to a registry, and use `image(…)`. See [Build images in CI](#build-images-in-ci). |
| Change the display stack, the scripts, or the ports | Define a profile. See [Define a profile](#define-a-profile). |

## Two builder methods

| Method | Source | The crate builds it | Packages |
| --- | --- | --- | --- |
| `image_dir(path)` | A directory with a `Dockerfile` | Yes, when the directory changes | Accepted |
| `image(tag)` | An image that exists, local or in a registry | No | Refused |

```rust
let computer = Computer::builder()
    .image_dir("images/my-desktop")
    .launch()
    .await?;

let computer = Computer::builder()
    .image("registry.example.com/desktop:1")
    .launch()
    .await?;
```

For `image_dir`, the image tag contains a hash of all files in the directory. A change to any file makes a new image. Packages from `packages([…])` go to the build as the `EXTRA_PACKAGES` build argument.

With `image`, the crate does not build anything, so it cannot add packages. `packages([…])` with `image(…)` fails with `Unsupported`.

## The image contract

The desktop driver runs fixed commands and connects to fixed ports in the image. An image must provide them, or the box does not start or some tools do not work.

The contract of the built-in X11 profile (`computer-desktop`):

| Item | Value |
| --- | --- |
| Boot command | `computer-desktop --once` |
| Screen command | `computer-screen` |
| Browser command | `computer-browser` |
| Screen size | From `COMPUTER_SCREEN_WIDTH` and `COMPUTER_SCREEN_HEIGHT`. Default 1280x800. |
| Watch viewer, screen N | Port `6080 + 2N` |
| Control viewer, screen N | Port `6081 + 2N` |
| Chrome DevTools | Port `9222`, with a bridge on `9223` |
| Screens | At most 8 |

The Wayland profile uses the name `computer-wayland`.

The easy way to meet the contract is to build `FROM` the built-in image, which already has all of it.

### Declare the contract

Put this label in the Dockerfile:

```dockerfile
LABEL computer.profile="computer-desktop"
```

Before it starts a box, the crate reads the label. If the image declares a different profile from the one that drives the box, the launch fails at once with the two names. With no label, the crate does not check, and a mismatch shows only as a failure later.

## Extend the built-in image

`examples/images/acme/Dockerfile` adds two packages and a file to the built-in image:

```dockerfile
ARG BASE=computer-desktop:unset
FROM ${BASE}

RUN apt-get update \
    && apt-get install -y --no-install-recommends htop jq \
    && rm -rf /var/lib/apt/lists/*

RUN mkdir -p /opt/acme \
    && printf 'built into the image, not written after launch\n' > /opt/acme/README

LABEL computer.profile="computer-desktop"
```

The built-in image tag changes when the crate's image source changes, so the example passes it in as `BASE`. Get the current tag in Rust:

```rust
use holm::bundle::{DESKTOP, Extras};

let base_tag = DESKTOP.tag_with(&Extras::none());
```

The built-in image must exist locally before `docker build` can use it. Launch one box first, or build it as in [Build images in CI](#build-images-in-ci).

Then build and use the new image:

```bash
docker build --build-arg BASE="$BASE_TAG" --tag acme-desktop:1 examples/images/acme
```

```rust
let computer = Computer::builder()
    .image("acme-desktop:1")
    .launch()
    .await?;
```

Run the full example:

```bash
cargo run --example custom_image
```

## Build images in CI

The first box with a new image waits for a build of several minutes. To avoid this, build the image in CI and push it to a registry.

The built-in Dockerfile and its scripts are in `crates/holm-core/images/desktop`. The directory is a complete Docker context:

```bash
docker build \
  --build-arg EXTRA_PACKAGES="jq ripgrep" \
  --tag registry.example.com/desktop:1 \
  crates/holm-core/images/desktop
docker push registry.example.com/desktop:1
```

`EXTRA_PACKAGES` is a list of apt packages, separated by spaces.

Then launch from the registry. The runtime pulls the image when the host does not have it:

```rust
let computer = Computer::builder()
    .image("registry.example.com/desktop:1")
    .launch()
    .await?;
```

Build the image again for each release of the crate that changes `crates/holm-core/images/`, so the scripts in the image match the driver.

For a server, `holm image build` builds the image that a runtime needs on the server's host. See [Deploy holmd](deploy-holmd.md#8-build-images-in-advance).

### Other contexts in the repository

`crates/holm-core/images` has other contexts that implement the same `computer-desktop` contract. Tests keep their scripts the same as the built-in ones.

| Directory | Base |
| --- | --- |
| `desktop` | Debian bookworm. The built-in image. |
| `ubuntu` | Ubuntu 24.04, with Chromium from the Xtradeb PPA |
| `tiny` | Debian bookworm, with documentation, locales, and other large files left out |
| `wayland` | The Wayland image. Contract `computer-wayland`. |

Use one with `image_dir`:

```rust
let computer = Computer::builder()
    .image_dir("crates/holm-core/images/ubuntu")
    .launch()
    .await?;
```

## Define a profile

A profile tells the driver how to use an image: its contract name, its image, its ports, its commands, its screen size, and what it supports. Use `ProfileBuilder` to change part of a built-in profile and keep the rest:

```rust
use holm::{CommandScreen, CommandWallpaperRuntime, ProfileBuilder, X11Profile};

let profile = ProfileBuilder::new(X11Profile)
    .name("my-desktop")
    .image_dir("images/mine")
    .screen_commands(CommandScreen::new("my-screen"))
    .wallpaper_runtime(CommandWallpaperRuntime::new("my-wallpaper"))
    .build();

let computer = Computer::builder()
    .profile(Arc::new(profile))
    .launch()
    .await?;
```

| Method | Changes |
| --- | --- |
| `name(…)` | The contract name. It must match the image's `computer.profile` label. |
| `image(ImageSource)`, `image_dir(path)` | The image. |
| `boot_command(…)` | The command that starts the desktop. |
| `screen_commands(…)` | The command that starts and stops a screen. |
| `browser_runtime(…)`, `wallpaper_runtime(…)`, `screen_runtime(…)` | How the driver opens the browser, sets the wallpaper, and runs screen tasks. |
| `ports(PortLayout)` | The viewer, VNC, and DevTools ports. |
| `geometry(GeometrySpec)` | The screen size variables and the default size. |
| `support(DesktopSupport)` | What the profile claims to support. |
| `driver(…)` | The desktop driver. |

The base can be `X11Profile` or `WaylandProfile`. A new display server needs its own implementations of the `Desktop` and `DesktopFactory` traits.

## Audit a desktop

A profile claims what it supports in its `DesktopSupport`. An audit tests those claims against a running box: the screen, the pointer, the DevTools connection, the clipboard, the viewer, and the takeover, where the profile claims them.

```rust
let audit = holm::audit(&computer).await;
println!("{audit}");
assert!(audit.ok());
```

`audit.met` lists the claims that work. `audit.unmet` lists the claims that do not work, with the reason. `audit.skipped` lists the checks that did not run, with the reason. The audit leaves the screen as it was, except for the pointer and the clipboard.

To fail on a gap, with a time limit:

```rust
let audit = holm::audit::audit_strictly(&computer, Duration::from_secs(120)).await?;
```

`audit_strictly` returns `Denied` with the report when a claim does not work, and `Timeout` when the audit does not finish in time.

Run an audit on a new image before you use it, and on one box after each deploy.
