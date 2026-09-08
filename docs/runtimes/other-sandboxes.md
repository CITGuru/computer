# Other cloud sandboxes

A container and a microVM put the desktop on this host. A cloud sandbox does
not, so a service on a small machine can hand out desktops with no container
runtime and no `/dev/kvm` of its own. The boundary is still a kernel the box
does not share.

`computer` ships one such integration — E2B, in `sandboxes::e2b` — and a seam
for the rest. `sandboxes::remote` is that seam. Implement seven calls against
Modal, Daytona, Fly, or whatever you already pay for, and the driver, the
screens, the takeover gate and the descriptor above them are the same code a
container runs.

`examples/custom_sandbox.rs` is a whole vendor in one file. It runs on this
host, so you can watch every call work before writing the same call against an
API you cannot see.

## The shape

```rust
use computer::sandboxes::remote::{self, RemoteApi, Sandbox, SandboxPlan};

struct Daytona { /* your HTTP client */ }

#[async_trait]
impl RemoteApi for Daytona {
    fn vendor(&self) -> &str { "daytona" }
    async fn available(&self) -> Result<()> { … }
    async fn create(&self, plan: &SandboxPlan) -> Result<Sandbox> { … }
    async fn find(&self, name: &str) -> Result<Option<Sandbox>> { … }
    async fn kill(&self, id: &str) -> Result<()> { … }
    async fn exec(&self, sandbox: &Sandbox, argv: &[String],
                  env: &BTreeMap<String, String>) -> Result<ExecResult> { … }
    async fn read(&self, sandbox: &Sandbox, path: &str) -> Result<Vec<u8>> { … }
    async fn write(&self, sandbox: &Sandbox, path: &str, bytes: &[u8]) -> Result<()> { … }
}

let (machine, profile) = remote::pair(Arc::new(Daytona::new()?), Arc::new(X11Profile));

let computer = Computer::builder()
    .machine(Arc::new(machine))
    .profile(profile)
    .image("your-snapshot")
    .launch()
    .await?;
```

`pair` builds the machine and the profile together because they share the cell
the sandbox lands in. A viewer URL contains an ID the vendor assigns after the
profile was built, and building the two apart gets that wrong quietly.

Nothing here needs a crate feature. The seam is a trait, and your HTTP client
is your own.

## The seven calls

| Call | What it must do |
| --- | --- |
| `vendor` | Name the vendor. It appears in every error and in `Computer::runtime`. |
| `available` | Say whether the control plane answers and the key is accepted. Asked before anything is created. |
| `create` | Make the sandbox, and answer with its ID and where each port is published. |
| `find` | Return the sandbox carrying this name, with its endpoints and its token. |
| `kill` | Take it away. |
| `exec` | Run one command with this environment set. |
| `read` / `write` | Move one file's bytes. |

`exec` is the one everything above depends on. Every click, every screenshot
and every screen that starts is a command, and the environment carries the
display — a vendor that drops it runs the whole session against screen zero.

## The defaults, and when to override them

Five more calls have defaults, and each default is the honest answer for a
vendor that lacks the thing rather than a placeholder:

- `keep_alive` — nothing, for a sandbox with no deadline. `RemoteMachine` calls
  it lazily, at half the TTL, so an implementation costs one round trip per
  half-lifetime and not one per click.
- `logs` — empty. This is where a screen that never came up explains itself, so
  implement it if the vendor has it.
- `carrying` and `sweepable` — no listing, so no sweep. `sweep_expired` over a
  vendor that lists nothing would report every box as already gone.
- `reaper` — no command, so a dropped handle leaves the sandbox running until
  its deadline. That is a fact about the vendor, not something to hide.
- `ensure_image` — refuses a container image this crate built, and says how to
  get across. Override it with `Ok(())` where the vendor pulls an OCI
  reference.

## Ports are the part to get right

Every runtime on this host forwards a container port to a host port. These
vendors do not: each publishes a port at an address of its own. So the map is
data rather than a rule.

```rust
Sandbox::new(id).published_as(plan.publish.clone(), |port, id| {
    format!("{port}-{id}.proxy.daytona.work")
})
```

`published_as` covers the pattern vendors. A vendor that names its own URLs —
Modal hands back a tunnel per port — fills `Sandbox::endpoints` directly. A
port missing from that map has no URL at all, which is the right answer for a
port the vendor never published.

`find` has to fill the same map. A box this process did not start has no viewer
otherwise.

## The viewer, and the gate in front of it

`public_viewer` is off by default, and **off is not privacy**. The vendor
publishes those ports at an address it chose. That address answers whether or
not this crate prints it.

What the setting decides is whether you are handed the URL. Off, the viewer
ports are left out of the port map and `viewer_url()` answers `None`; the
desktop is still driveable from your program.

```rust
Computer::builder()
    .machine(Arc::new(machine.public_viewer(true)))
    .auth(Auth::Password)
```

On, `Machine::reach` says `Routable`, so launch refuses an open viewer the same
way it refuses `Bind::Any`. The control port drives the desktop, and it would
accept anyone who reached it.

Whether the vendor's own proxy gates that URL is the vendor's decision. Check
it rather than assume it — measure a request with no credentials and see what
comes back.

## What does not travel

- **DevTools.** An endpoint out here is `wss` on a public host and this crate's
  DevTools client speaks plain TCP. `RemoteProfile` drops the bridge port and
  clears the `cdp` claim, so `devtools()` returns `None` and `audit` skips the
  check rather than failing it. Synthetic input, screenshots, the clipboard,
  the viewer and the takeover are untouched.
- **The image.** This crate builds container images; these vendors run a
  template or a snapshot they built themselves. Write the build context out
  with `Bundle::materialize`, build it there with
  `/usr/local/bin/computer-desktop` as the start command, and pass what the
  vendor calls the result to `Builder::image`. `images/context.py` handles the
  Dockerfile subsets each builder accepts.
- **Packages.** `Builder::packages` folds them into a build, and there is
  no build here. `start` refuses it rather than starting a box without them.
- **The entrypoint.** A sandbox comes up empty, so `RemoteMachine` runs the
  profile's boot command itself. A vendor whose image starts its own screen
  will refuse the second one.

## Two vendors, concretely

**Daytona** is HTTP and JSON, so a Rust adapter is direct: create a sandbox
with your labels, list by label for `find`, and use the toolbox endpoints for
`exec` and for files. Preview links are `{port}-{id}.proxy.daytona.work`, which
`published_as` formats. Its auto-stop interval is the deadline `keep_alive`
pushes out. Check the current paths against Daytona's API reference before you
write them down.

**Modal** has no Rust client, and its sandbox control plane is gRPC behind a
Python API. The seam still fits, but the calls go to a Modal web endpoint of
your own that creates the sandbox and returns its ID and its tunnel URLs. That
endpoint wraps `Sandbox.create`, `sandbox.exec`, `sandbox.open` and
`Tunnel.url`; your `RemoteApi` is then ordinary HTTP.

## Testing it without an account

`computer::testing::ScriptedRemote` is a `RemoteApi` with no cloud behind it. A
vendor adapter is mostly a request body and a response shape, and both are
checkable in milliseconds:

```rust
let api = Arc::new(ScriptedRemote::new());
let (machine, _) = remote::pair(Arc::clone(&api) as Arc<dyn RemoteApi>, Arc::new(X11Profile));

machine.start("desk-1", &config).await?;

assert_eq!(api.plans()[0].metadata[NAME_KEY], "desk-1");
assert_eq!(api.commands(), vec![vec!["computer-desktop", "--once"]]);
```

It records plans, commands, kills, deadline refreshes, lookups and writes.
`holding(name, id)` says a box already exists, as one left by another process
would.

## Before you ship it

- `find` fills `endpoints`, or a box picked up later has no viewer.
- `exec` passes the environment through.
- `create` stores the plan's metadata where `carrying` can read it back, or no
  sweep will ever find the box.
- The image was built for the vendor, not pulled from this crate's bundle.
- You measured what an ungated viewer URL answers.
