//! A box in a virtual machine on Apple hardware.

pub mod lume;
pub mod rfb;
pub mod tart;

pub use lume::Lume;
pub use tart::Tart;

use crate::error::{Error, Result};
use crate::exec::ExecResult;
use crate::machine::{Machine, PortMap};
use crate::microvm::{free_port, mebibytes, port_pairs};
use crate::runtime::Config;
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// How many macOS guests one host may run at once.
pub const MAX_GUESTS: usize = 2;

/// What a macOS desktop needs when the caller names no ceiling.
pub const DESKTOP_MEMORY_MIB: u64 = 8192;

/// How long to wait for a guest to be handed an address.
pub const ADDRESS_WAIT: Duration = Duration::from_secs(120);

/// How long to wait for `tart run` to announce its viewer.
pub const VIEWER_WAIT: Duration = Duration::from_secs(60);

/// The port a macOS box's viewer is recorded against.
pub const VIEWER_PORT: u16 = crate::image::VIEW_PORT_BASE;

/// What to say to one virtual machine runtime.
pub trait Backend: Send + Sync {
    /// The program to run, for the message when it is missing.
    fn program(&self) -> &str;

    fn clone_args(&self, plan: &Plan) -> Vec<String>;

    /// Give the clone its own identity and hardware, before it boots.
    fn shape_args(&self, plan: &Plan) -> Vec<String>;

    fn run_args(&self, plan: &Plan) -> Vec<String>;

    fn exec_args(&self, name: &str, argv: &[String], env: &BTreeMap<String, String>)
    -> Vec<String>;

    /// Take a file on standard input and write it in the guest.
    fn write_args(&self, name: &str, path: &str) -> Vec<String>;

    fn address_args(&self, name: &str) -> Vec<String>;
    fn list_args(&self) -> Vec<String>;
    fn stop_args(&self, name: &str) -> Vec<String>;
    fn delete_args(&self, name: &str) -> Vec<String>;
    fn pull_args(&self, image: &str) -> Vec<String>;
    fn version_args(&self) -> Vec<String>;

    /// The viewer this runtime was *told* to serve, where it can be told.
    fn announced_viewer(&self, _plan: &Plan) -> Option<Viewer> {
        None
    }

    /// The viewer this runtime announced, from one line of its output.
    fn parse_viewer(&self, line: &str) -> Option<Viewer>;

    /// The guest's address, from whatever the address command printed.
    fn parse_address(&self, output: &str) -> Option<String>;

    /// How many macOS guests are running, from whatever the listing printed.
    fn running_guests(&self, listing: &str) -> Option<usize>;
}

/// How to run one, as opposed to what to say. See [`Backend`].
#[async_trait]
pub trait VmCli: Send + Sync {
    /// Run the program with these arguments and wait for it.
    async fn run(&self, args: &[String]) -> Result<ExecResult>;

    /// The same, with `bytes` on the command's standard input.
    async fn run_with_stdin(&self, args: &[String], bytes: &[u8]) -> Result<ExecResult>;

    /// Start `tart` with these arguments and leave it running.
    async fn spawn(
        &self,
        args: &[String],
        announced: &(dyn for<'a> Fn(&'a str) -> bool + Send + Sync),
    ) -> Result<Option<String>>;

    /// The program being run, for the message when it is missing.
    fn program(&self) -> &str;
}

/// The `tart` on this host.
#[derive(Debug, Clone)]
pub struct SystemTart {
    program: String,
}

impl Default for SystemTart {
    fn default() -> Self {
        Self::new("tart")
    }
}

impl SystemTart {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
        }
    }

    fn missing(&self, error: std::io::Error) -> Error {
        match error.kind() {
            std::io::ErrorKind::NotFound => Error::Unavailable {
                runtime: self.program.clone(),
                detail: format!("{} is not on PATH", self.program),
            },
            _ => Error::transport(error.to_string(), false),
        }
    }
}

#[async_trait]
impl VmCli for SystemTart {
    async fn run(&self, args: &[String]) -> Result<ExecResult> {
        let output = tokio::process::Command::new(&self.program)
            .args(args)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .map_err(|error| self.missing(error))?;

        Ok(ExecResult {
            code: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
            timed_out: false,
        })
    }

    async fn run_with_stdin(&self, args: &[String], bytes: &[u8]) -> Result<ExecResult> {
        use tokio::io::AsyncWriteExt;

        let mut child = tokio::process::Command::new(&self.program)
            .args(args)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| self.missing(error))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(bytes)
                .await
                .map_err(|error| Error::transport(error.to_string(), false))?;
            // Dropped rather than left open: `cat` in the guest reads until
            // end of file, and a pipe still held is a command that never ends.
            drop(stdin);
        }

        let output = child
            .wait_with_output()
            .await
            .map_err(|error| Error::transport(error.to_string(), false))?;

        Ok(ExecResult {
            code: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
            timed_out: false,
        })
    }

    async fn spawn(
        &self,
        args: &[String],
        announced: &(dyn for<'a> Fn(&'a str) -> bool + Send + Sync),
    ) -> Result<Option<String>> {
        use tokio::io::AsyncBufReadExt;

        // Deliberately not `kill_on_drop`: this process owning the VM's
        // lifetime would take the box away with the future that started it.
        let mut child = tokio::process::Command::new(&self.program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| self.missing(error))?;

        let Some(stdout) = child.stdout.take() else {
            return Ok(None);
        };
        let mut lines = tokio::io::BufReader::new(stdout).lines();
        // Read only until the announcement. Waiting for end of file would
        // wait for the VM to shut down.
        let found = {
            let reader = &mut lines;
            tokio::time::timeout(VIEWER_WAIT, async move {
                while let Ok(Some(line)) = reader.next_line().await {
                    // The borrow ends before the move: the predicate only
                    // reads the line, and the caller wants it back.
                    let matched = announced(line.as_str());
                    if matched {
                        return Some(line);
                    }
                }
                None
            })
            .await
            .ok()
            .flatten()
        };

        // Both pipes are drained for the VM's whole life. A pipe nobody reads
        // fills at the operating system's buffer and blocks the writer, and a
        // blocked `tart run` takes the guest down with it — which looks like a
        // box that dies on its own a minute after it started.
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut said = tokio::io::BufReader::new(stderr).lines();
                while let Ok(Some(line)) = said.next_line().await {
                    tracing::debug!(line, "tart");
                }
            });
        }

        tokio::spawn(async move {
            while let Ok(Some(_)) = lines.next_line().await {}
            let _ = child.wait().await;
        });

        Ok(found)
    }

    fn program(&self) -> &str {
        &self.program
    }
}

/// Which server a box's viewer is served by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewerMode {
    /// `Virtualization.framework`'s own server, running on this host.
    #[default]
    Framework,
    /// Apple Screen Sharing, running inside the guest.
    ScreenSharing,
    /// No viewer at all.
    None,
}

impl ViewerMode {
    /// The flag `tart run` takes for this, where it takes one.
    pub fn flag(self) -> Option<&'static str> {
        match self {
            Self::Framework => Some("--vnc-experimental"),
            Self::ScreenSharing => Some("--vnc"),
            Self::None => None,
        }
    }
}

/// Where the viewer for a running box is, as `tart run` announces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Viewer {
    pub host: String,
    pub port: u16,
    pub password: crate::Secret,
}

/// What to clone and run, decided before anything exists.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub name: String,
    /// The prepared image to clone from. Never built; see [`Machine::ensure_image`].
    pub base: String,
    pub cpus: Option<u8>,
    pub memory_mib: Option<u64>,
    pub network: bool,
    /// The size to pin the guest's display to.
    pub display: Option<(u32, u32)>,
    /// Which server answers the viewer, if any.
    pub viewer: ViewerMode,
    /// The port the viewer should listen on, where the runtime can be told.
    pub viewer_port: Option<u16>,
    /// The credential the viewer should ask for, where the runtime can be told.
    pub viewer_secret: Option<crate::Secret>,
    /// Host port to guest port, chosen before the guest exists.
    pub ports: Vec<(u16, u16)>,
}

pub fn plan_for(name: &str, config: &Config, ports: Vec<(u16, u16)>, viewer: ViewerMode) -> Plan {
    Plan {
        name: name.to_string(),
        base: config.image.clone(),
        cpus: config.cpus.as_ref().and_then(|cpus| cpus.parse().ok()),
        memory_mib: config
            .memory
            .as_deref()
            .and_then(mebibytes)
            .or(Some(DESKTOP_MEMORY_MIB)),
        network: config.network,
        display: Some((config.width, config.height)),
        viewer,
        viewer_port: None,
        viewer_secret: None,
        ports,
    }
}

fn arg(value: impl Into<String>) -> String {
    value.into()
}

/// One argument, safe inside the single-quoted shell `exec_args` builds.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// One viewer connection: input dropped going in, pixels untouched coming out.
async fn watch_only(near: tokio::net::TcpStream, far: tokio::net::TcpStream) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (mut watcher, mut to_watcher) = near.into_split();
    let (mut from_server, mut to_server) = far.into_split();

    // Shared, because under RFB 3.3 it is the server that names the security
    // type and the client stream alone cannot then be framed. See
    // [`rfb::ViewOnly::server_said`].
    let filter = Arc::new(Mutex::new(rfb::ViewOnly::new()));

    let watching = Arc::clone(&filter);
    let pixels = tokio::spawn(async move {
        let mut buffer = [0u8; 8192];
        while let Ok(read) = from_server.read(&mut buffer).await {
            if read == 0 {
                break;
            }
            if let Ok(mut filter) = watching.lock() {
                filter.server_said(&buffer[..read]);
            }
            if to_watcher.write_all(&buffer[..read]).await.is_err() {
                break;
            }
        }
    });

    let mut buffer = [0u8; 8192];
    // Whether the client stopped sending but may still be reading. A viewer
    // that has said everything it means to say is not a viewer that has left.
    let mut half_closed = false;

    loop {
        let read = match watcher.read(&mut buffer).await {
            Ok(0) => {
                half_closed = true;
                break;
            }
            Ok(read) => read,
            Err(_) => break,
        };

        // Held bytes wait for the phase the server is about to announce. The
        // client cannot answer a challenge it has not been sent, so more of
        // its bytes always follow and the wait always ends.
        let allowed = match filter.lock() {
            Ok(mut filter) => filter.filter(&buffer[..read]),
            Err(_) => break,
        };

        match allowed {
            Ok(allowed) if allowed.is_empty() => {}
            Ok(allowed) => {
                if to_server.write_all(&allowed).await.is_err() {
                    break;
                }
            }
            Err(error) => {
                tracing::warn!(%error, "a viewer connection stopped making sense");
                break;
            }
        }
    }

    if half_closed {
        // Pass the half-close on and keep the pixels coming until the server
        // ends it. Cutting the picture here would end a session the client
        // never asked to end.
        let _ = to_server.shutdown().await;
        let _ = pixels.await;
    } else {
        pixels.abort();
    }
}

/// A box in a macOS guest under Tart.
pub struct MacMachine {
    backend: Arc<dyn Backend>,
    cli: Arc<dyn VmCli>,
    /// What was relayed, per box. The pairs were chosen here, so this side
    /// already knows them and does not ask for them back.
    published: Mutex<BTreeMap<String, PortMap>>,
    started_with: Mutex<BTreeMap<String, BTreeMap<String, String>>>,
    /// Which server answers the viewer for boxes started here.
    viewer: ViewerMode,
    /// The relays carrying those ports, held so `stop` can end them.
    relays: Mutex<BTreeMap<String, Vec<tokio::task::JoinHandle<()>>>>,
    /// Where each box's viewer is, as `tart run` announced it once.
    viewers: Mutex<BTreeMap<String, Viewer>>,
}

impl Default for MacMachine {
    fn default() -> Self {
        Self::new(Arc::new(SystemTart::default()))
    }
}

impl MacMachine {
    /// A machine driving `tart`, which is the backend this crate began with.
    pub fn new(cli: Arc<dyn VmCli>) -> Self {
        Self::with_backend(Arc::new(Tart), cli)
    }

    /// A machine driving whichever runtime `backend` speaks for.
    pub fn with_backend(backend: Arc<dyn Backend>, cli: Arc<dyn VmCli>) -> Self {
        Self {
            backend,
            cli,
            published: Mutex::new(BTreeMap::new()),
            started_with: Mutex::new(BTreeMap::new()),
            viewer: ViewerMode::default(),
            relays: Mutex::new(BTreeMap::new()),
            viewers: Mutex::new(BTreeMap::new()),
        }
    }

    /// Serve the viewer from this, rather than the default.
    pub fn viewing_with(mut self, mode: ViewerMode) -> Self {
        self.viewer = mode;
        self
    }

    /// Start boxes with no viewer.
    pub fn without_viewer(self) -> Self {
        self.viewing_with(ViewerMode::None)
    }

    /// Where this box's viewer is, and the credential that opens it.
    pub fn viewer(&self, name: &str) -> Option<Viewer> {
        self.viewers
            .lock()
            .ok()
            .and_then(|viewers| viewers.get(name).cloned())
    }

    pub fn cli(&self) -> &Arc<dyn VmCli> {
        &self.cli
    }

    pub fn backend(&self) -> &Arc<dyn Backend> {
        &self.backend
    }

    async fn tart(&self, args: &[String]) -> Result<ExecResult> {
        self.cli.run(args).await
    }

    /// Whether this host already holds as many macOS guests as it may run.
    async fn at_capacity(&self) -> bool {
        let listed = self
            .tart(&[arg("list"), arg("--format"), arg("json")])
            .await
            .ok();

        listed
            .filter(|result| result.code == 0)
            .and_then(|result| self.backend.running_guests(&result.stdout_utf8()))
            .is_some_and(|running| running >= MAX_GUESTS)
    }

    /// The guest's address, once it has one.
    async fn address(&self, name: &str) -> Result<String> {
        let deadline = SystemTime::now() + ADDRESS_WAIT;

        loop {
            if let Ok(result) = self.tart(&[arg("ip"), arg(name)]).await
                && result.code == 0
                && let Some(address) = self.backend.parse_address(&result.stdout_utf8())
            {
                return Ok(address);
            }

            if SystemTime::now() >= deadline {
                return Err(Error::Unavailable {
                    runtime: self.cli.program().to_string(),
                    detail: format!("{name} was never given an address"),
                });
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// Carry one host port to the guest for as long as the box lives.
    async fn relay(&self, host_port: u16, to: String) -> Result<tokio::task::JoinHandle<()>> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", host_port))
            .await
            .map_err(|error| Error::transport(format!("port {host_port}: {error}"), false))?;

        Ok(tokio::spawn(async move {
            loop {
                let Ok((mut near, _)) = listener.accept().await else {
                    return;
                };
                let there = to.clone();

                tokio::spawn(async move {
                    let Ok(mut far) = tokio::net::TcpStream::connect(&there).await else {
                        return;
                    };
                    // Both halves until either end hangs up. Nothing here reads
                    // the bytes: this is a wire, not a proxy.
                    let _ = tokio::io::copy_bidirectional(&mut near, &mut far).await;
                });
            }
        }))
    }

    /// Carry the viewer port with the input taken out of it.
    async fn view_relay(&self, host_port: u16, to: String) -> Result<tokio::task::JoinHandle<()>> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", host_port))
            .await
            .map_err(|error| Error::transport(format!("port {host_port}: {error}"), false))?;

        Ok(tokio::spawn(async move {
            loop {
                let Ok((near, _)) = listener.accept().await else {
                    return;
                };
                let there = to.clone();

                tokio::spawn(async move {
                    let Ok(far) = tokio::net::TcpStream::connect(&there).await else {
                        return;
                    };
                    watch_only(near, far).await;
                });
            }
        }))
    }

    fn remember(&self, name: &str, plan: &Plan, env: &BTreeMap<String, String>) {
        if let Ok(mut published) = self.published.lock() {
            published.insert(
                name.to_string(),
                plan.ports
                    .iter()
                    .map(|(host, guest)| (*guest, *host))
                    .collect(),
            );
        }
        if let Ok(mut started) = self.started_with.lock() {
            started.insert(name.to_string(), env.clone());
        }
    }

    fn end_relays(&self, name: &str) {
        if let Ok(mut viewers) = self.viewers.lock() {
            viewers.remove(name);
        }
        if let Ok(mut relays) = self.relays.lock()
            && let Some(handles) = relays.remove(name)
        {
            for handle in handles {
                handle.abort();
            }
        }
    }
}

#[async_trait]
impl Machine for MacMachine {
    fn runtime(&self) -> &str {
        self.cli.program()
    }

    async fn preflight(&self) -> Result<()> {
        // The licence permits a macOS guest only on Apple hardware, and the
        // framework enforces it. Refused here so it reads as the rule it is
        // rather than as a runtime that would not start.
        if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            return Err(Error::Unsupported {
                gaps: vec!["a macOS guest needs an Apple silicon host"],
            });
        }

        let alive = self.tart(&[arg("--version")]).await?;
        if alive.code != 0 {
            return Err(Error::Unavailable {
                runtime: self.runtime().to_string(),
                detail: alive.stderr_utf8().trim().to_string(),
            });
        }
        Ok(())
    }

    /// Fetch the prepared image, and never build one.
    async fn ensure_image(&self, config: &Config) -> Result<()> {
        if config.bundle.is_some() || config.image_dir.is_some() {
            return Err(Error::Unsupported {
                gaps: vec!["building a macOS image: prepare one and push it"],
            });
        }
        if !config.extras.is_empty() {
            return Err(Error::Unsupported {
                gaps: vec!["packages in an image this crate does not build"],
            });
        }

        let pulled = self.tart(&[arg("pull"), config.image.clone()]).await?;
        if pulled.code != 0 {
            return Err(Error::Unavailable {
                runtime: self.runtime().to_string(),
                detail: pulled.stderr_utf8().trim().to_string(),
            });
        }
        Ok(())
    }

    async fn start(&self, name: &str, config: &Config) -> Result<PortMap> {
        if self.at_capacity().await {
            return Err(Error::Unavailable {
                runtime: self.runtime().to_string(),
                detail: format!(
                    "this host already runs {MAX_GUESTS} macOS guests, which is \
                     all the virtualisation framework allows"
                ),
            });
        }

        let mut plan = plan_for(
            name,
            config,
            port_pairs(&config.publish, free_port),
            self.viewer,
        );

        if plan.viewer != ViewerMode::None {
            // Offered to every backend. The one that cannot be told ignores
            // both and announces its own on the way up instead.
            plan.viewer_port = free_port();
            plan.viewer_secret = crate::Secret::generate().ok();
        }

        for args in [
            self.backend.clone_args(&plan),
            self.backend.shape_args(&plan),
        ] {
            let step = self.tart(&args).await?;
            if step.code != 0 {
                let _ = self.stop(name).await;
                return Err(Error::Unavailable {
                    runtime: self.runtime().to_string(),
                    detail: step.stderr_utf8().trim().to_string(),
                });
            }
        }

        let backend = Arc::clone(&self.backend);
        let announced = self
            .cli
            .spawn(&self.backend.run_args(&plan), &move |line: &str| {
                backend.parse_viewer(line).is_some()
            })
            .await?;

        // What the backend was told beats what it said: a runtime that takes a
        // port and a secret has already been given both.
        let announced = self
            .backend
            .announced_viewer(&plan)
            .or_else(|| announced.and_then(|line| self.backend.parse_viewer(&line)));

        let address = match self.address(name).await {
            Ok(address) => address,
            Err(error) => {
                let _ = self.stop(name).await;
                return Err(error);
            }
        };

        let mut carried = Vec::new();
        for (host_port, guest_port) in &plan.ports {
            match self
                .relay(*host_port, format!("{address}:{guest_port}"))
                .await
            {
                Ok(handle) => carried.push(handle),
                Err(error) => {
                    for handle in carried {
                        handle.abort();
                    }
                    let _ = self.stop(name).await;
                    return Err(error);
                }
            }
        }

        // The viewer is not a guest port, so it is not in `plan.ports`: it is
        // a server on this host, reached through a filter rather than
        // directly, and recorded under the port everything above looks for.
        let mut viewer_map = None;
        if let Some(viewer) = announced {
            let Some(host_port) = free_port() else {
                for handle in carried {
                    handle.abort();
                }
                let _ = self.stop(name).await;
                return Err(Error::transport("no free port for the viewer", false));
            };

            match self
                .view_relay(host_port, format!("{}:{}", viewer.host, viewer.port))
                .await
            {
                Ok(handle) => {
                    carried.push(handle);
                    viewer_map = Some((VIEWER_PORT, host_port));
                    if let Ok(mut viewers) = self.viewers.lock() {
                        viewers.insert(name.to_string(), viewer);
                    }
                }
                Err(error) => {
                    for handle in carried {
                        handle.abort();
                    }
                    let _ = self.stop(name).await;
                    return Err(error);
                }
            }
        }

        if let Ok(mut relays) = self.relays.lock() {
            relays.insert(name.to_string(), carried);
        }
        self.remember(name, &plan, &config.env);

        if let Some((guest, host)) = viewer_map
            && let Ok(mut published) = self.published.lock()
        {
            published
                .entry(name.to_string())
                .or_default()
                .insert(guest, host);
        }

        Ok(self.ports(name).await)
    }

    async fn running(&self, name: &str) -> Result<bool> {
        let listed = self
            .tart(&[arg("list"), arg("--format"), arg("json")])
            .await?;

        Ok(listed.stdout_utf8().contains(name))
    }

    async fn ports(&self, name: &str) -> PortMap {
        self.published
            .lock()
            .ok()
            .and_then(|published| published.get(name).cloned())
            .unwrap_or_default()
    }

    async fn env(&self, name: &str) -> BTreeMap<String, String> {
        self.started_with
            .lock()
            .ok()
            .and_then(|started| started.get(name).cloned())
            .unwrap_or_default()
    }

    async fn exec(
        &self,
        name: &str,
        argv: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<ExecResult> {
        self.tart(&self.backend.exec_args(name, argv, env)).await
    }

    async fn read_file(&self, name: &str, path: &Path) -> Result<Vec<u8>> {
        let read = self
            .exec(
                name,
                &[arg("cat"), arg(path.display().to_string())],
                &BTreeMap::new(),
            )
            .await?;

        if read.code != 0 {
            return Err(Error::denied(read.stderr_utf8().trim().to_string()));
        }
        Ok(read.stdout)
    }

    /// Bytes in on standard input, not on a command line.
    async fn write_file(&self, name: &str, path: &Path, bytes: &[u8]) -> Result<()> {
        let quoted = shell_quote(&path.display().to_string());
        let mut args = vec![arg("exec"), arg("-i"), arg(name), arg("sh"), arg("-c")];
        args.push(format!(
            "mkdir -p \"$(dirname {quoted})\" && cat > {quoted}"
        ));

        let written = self.cli.run_with_stdin(&args, bytes).await?;
        if written.code != 0 {
            return Err(Error::denied(written.stderr_utf8().trim().to_string()));
        }
        Ok(())
    }

    async fn logs(&self, _name: &str) -> Result<String> {
        // `tart run` was detached, so nothing here holds its output.
        Ok(String::new())
    }

    async fn stop(&self, name: &str) -> Result<()> {
        self.end_relays(name);

        // Stopped then deleted: a clone left behind holds its disk, and a host
        // with two of them cannot start a third box.
        let _ = self.tart(&[arg("stop"), arg(name)]).await;

        let deleted = self.tart(&[arg("delete"), arg(name)]).await?;
        if deleted.code != 0 {
            return Err(Error::transport(
                deleted.stderr_utf8().trim().to_string(),
                true,
            ));
        }
        Ok(())
    }

    /// Tart holds no labels, so nothing can be swept.
    fn sweepable(&self) -> bool {
        false
    }

    fn reaper(&self, name: &str) -> Option<(String, Vec<String>)> {
        Some((
            "/bin/sh".to_string(),
            vec![
                arg("-c"),
                format!(
                    "{program} stop {name}; {program} delete {name}",
                    program = self.cli.program()
                ),
            ],
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct ScriptedTart {
        calls: Mutex<Vec<Vec<String>>>,
        spawned: Mutex<Vec<Vec<String>>>,
        written: Mutex<Vec<Vec<u8>>>,
        saying: String,
    }

    impl ScriptedTart {
        fn saying(listing: impl Into<String>) -> Self {
            Self {
                saying: listing.into(),
                ..Self::default()
            }
        }
    }

    #[async_trait]
    impl VmCli for ScriptedTart {
        async fn run(&self, args: &[String]) -> Result<ExecResult> {
            if let Ok(mut calls) = self.calls.lock() {
                calls.push(args.to_vec());
            }
            Ok(ExecResult {
                stdout: self.saying.clone().into_bytes(),
                ..ExecResult::default()
            })
        }

        async fn run_with_stdin(&self, args: &[String], bytes: &[u8]) -> Result<ExecResult> {
            if let Ok(mut written) = self.written.lock() {
                written.push(bytes.to_vec());
            }
            self.run(args).await
        }

        async fn spawn(
            &self,
            args: &[String],
            _announced: &(dyn for<'a> Fn(&'a str) -> bool + Send + Sync),
        ) -> Result<Option<String>> {
            if let Ok(mut spawned) = self.spawned.lock() {
                spawned.push(args.to_vec());
            }
            Ok(None)
        }

        fn program(&self) -> &str {
            "tart"
        }
    }

    fn plan() -> Plan {
        plan_for(
            "box",
            &Config {
                image: "ghcr.io/cirruslabs/macos-sequoia-base:latest".to_string(),
                publish: vec![6080],
                ..Config::default()
            },
            vec![(50000, 6080)],
            ViewerMode::default(),
        )
    }

    #[test]
    fn test_each_server_has_its_own_flag_and_they_are_not_the_same_one() {
        assert_eq!(ViewerMode::Framework.flag(), Some("--vnc-experimental"));
        assert_eq!(
            ViewerMode::ScreenSharing.flag(),
            Some("--vnc"),
            "plain --vnc needs Remote Login inside the guest; they are two \
             different servers and not two names for one"
        );
        assert_eq!(ViewerMode::None.flag(), None);
        assert_eq!(
            ViewerMode::default(),
            ViewerMode::Framework,
            "the host-side one answers before the guest has logged in, which \
             is when a box that came up wrong is worth looking at"
        );
    }

    #[tokio::test]
    async fn test_a_third_box_is_refused_before_the_framework_refuses_it() {
        let full = r#"[
            {"Name":"one","State":"running","OS":"darwin"},
            {"Name":"two","State":"running","OS":"darwin"}
        ]"#;
        let machine = MacMachine::new(Arc::new(ScriptedTart::saying(full)));

        let error = machine
            .start("third", &Config::default())
            .await
            .expect_err("the host is full");

        assert!(
            error.needs_another_place(),
            "a scheduler has to move to another Mac rather than retry here"
        );
        assert!(error.to_string().contains("2"), "{error}");
    }

    #[tokio::test]
    async fn test_a_macos_image_is_never_built() {
        let machine = MacMachine::new(Arc::new(ScriptedTart::default()));
        let config = Config {
            bundle: Some(crate::bundle::DESKTOP),
            ..Config::default()
        };

        assert!(
            machine.ensure_image(&config).await.is_err(),
            "there is no building a macOS guest on first use, and pulling one \
             that was never pushed waits for something that cannot arrive"
        );
    }

    #[tokio::test]
    async fn test_bytes_go_in_on_stdin_and_not_through_upload() {
        let cli = Arc::new(ScriptedTart::default());
        let machine = MacMachine::new(Arc::clone(&cli) as Arc<dyn VmCli>);

        machine
            .write_file("box", Path::new("/tmp/probe"), b"round trip")
            .await
            .expect("a write");

        let sent = cli.calls.lock().expect("the calls").last().cloned();
        let sent = sent.expect("a call");
        assert_eq!(sent[..3], ["exec", "-i", "box"], "{sent:?}");
        assert!(
            sent.last().is_some_and(|part| part.contains("cat >")),
            "tart has no cp, and bytes on a command line have a ceiling a \
             screenshot walks through: {sent:?}"
        );
        assert_eq!(
            cli.written.lock().expect("the writes").last().cloned(),
            Some(b"round trip".to_vec()),
            "Machine::upload's default reads a file and calls write_file, so \
             a write_file that delegated to it would recurse until the stack \
             ran out"
        );
    }

    #[test]
    fn test_a_dropped_handle_takes_the_clone_and_not_just_the_guest() {
        let machine = MacMachine::new(Arc::new(ScriptedTart::default()));
        let (program, args) = machine.reaper("box").expect("a reaper");

        assert_eq!(program, "/bin/sh");
        assert!(args[1].contains("stop box"));
        assert!(
            args[1].contains("delete box"),
            "a stopped clone still holds its disk, and a host holding two of \
             them cannot start a third box"
        );
    }

    #[test]
    fn test_a_desktop_gets_enough_memory_even_when_nobody_asks() {
        assert_eq!(plan().memory_mib, Some(DESKTOP_MEMORY_MIB));
        assert_eq!(
            plan_for(
                "box",
                &Config {
                    memory: Some("4g".to_string()),
                    ..Config::default()
                },
                Vec::new(),
                ViewerMode::default()
            )
            .memory_mib,
            Some(4096)
        );
    }
}
