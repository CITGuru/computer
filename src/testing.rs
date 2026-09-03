//! Doubles, so code that drives a desktop can be tested without one.
//!
//! Each records what it was asked for and answers from a script, so a decision
//! about where to click is checked in milliseconds. The container tests are
//! left to prove the image.

use crate::error::{Error, Result};
use crate::machine::MachineHost;
use crate::machine::ScreenHost;
use crate::microvm::{MicroVmApi, Plan};
use crate::profile::{CommandScreen, ImageSource, PortLayout, Profile, ScreenCommands};
use crate::runtime::ContainerCli;
use crate::sandboxes::e2b::{self, E2bApi, Sandbox, SandboxPlan};
use crate::screens::ControlGate;
use crate::{
    Button, Delta, Desktop, DesktopFactory, DesktopSupport, Display, DisplayServer, ExecResult,
    Point, ScreenAction, ScreenId,
};
use async_trait::async_trait;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// An [`ScreenHost`] that records every command and answers from a queue.
pub struct ScriptedHost {
    calls: Mutex<Vec<(ScreenId, Vec<String>)>>,
    replies: Mutex<VecDeque<ExecResult>>,
    fallback: ExecResult,
}

impl Default for ScriptedHost {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptedHost {
    /// Answers every command with a clean exit and no output.
    pub fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            replies: Mutex::new(VecDeque::new()),
            fallback: ExecResult::default(),
        }
    }

    /// Queue one answer. They are handed out in order, and the fallback takes
    /// over once the queue is empty.
    pub fn replying(self, result: ExecResult) -> Self {
        if let Ok(mut replies) = self.replies.lock() {
            replies.push_back(result);
        }
        self
    }

    /// Queue a clean exit carrying this on standard output.
    pub fn saying(self, stdout: impl Into<String>) -> Self {
        self.replying(ExecResult {
            stdout: stdout.into().into_bytes(),
            ..ExecResult::default()
        })
    }

    /// Queue a failure.
    pub fn failing(self, code: i32, stderr: impl Into<String>) -> Self {
        self.replying(ExecResult {
            code,
            stderr: stderr.into().into_bytes(),
            ..ExecResult::default()
        })
    }

    /// What every command after the queue gets.
    pub fn otherwise(mut self, result: ExecResult) -> Self {
        self.fallback = result;
        self
    }

    /// Every command sent, in order.
    pub fn calls(&self) -> Vec<Vec<String>> {
        self.calls
            .lock()
            .map(|calls| calls.iter().map(|(_, argv)| argv.clone()).collect())
            .unwrap_or_default()
    }

    /// Which screen each command went to.
    pub fn screens(&self) -> Vec<ScreenId> {
        self.calls
            .lock()
            .map(|calls| calls.iter().map(|(screen, _)| *screen).collect())
            .unwrap_or_default()
    }

    pub fn last(&self) -> Option<Vec<String>> {
        self.calls().pop()
    }

    /// The last command as one string, for a readable assertion.
    pub fn last_line(&self) -> String {
        self.last().unwrap_or_default().join(" ")
    }

    pub fn count(&self) -> usize {
        self.calls.lock().map(|calls| calls.len()).unwrap_or(0)
    }

    fn record(&self, screen: ScreenId, argv: &[String]) -> ExecResult {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push((screen, argv.to_vec()));
        }
        self.replies
            .lock()
            .ok()
            .and_then(|mut replies| replies.pop_front())
            .unwrap_or_else(|| self.fallback.clone())
    }
}

#[async_trait]
impl ScreenHost for ScriptedHost {
    async fn run(&self, argv: &[String], screen: ScreenId) -> Result<ExecResult> {
        Ok(self.record(screen, argv))
    }
}

/// A [`Desktop`] with no display server behind it.
///
/// A second implementation of the trait, showing what a driver has to supply
/// without an X server, a compositor or a container behind it.
pub struct ScriptedDesktop {
    screen: ScreenId,
    control: Arc<ControlGate>,
    acted: Mutex<Vec<String>>,
}

impl ScriptedDesktop {
    pub fn new(screen: ScreenId) -> Self {
        Self {
            screen,
            control: Arc::new(ControlGate::new()),
            acted: Mutex::new(Vec::new()),
        }
    }

    pub fn screen(&self) -> ScreenId {
        self.screen
    }

    /// What it was asked to do, in order.
    pub fn acted(&self) -> Vec<String> {
        self.acted
            .lock()
            .map(|acted| acted.clone())
            .unwrap_or_default()
    }

    fn act(&self, what: impl Into<String>) -> Result<()> {
        self.control.may_act()?;
        if let Ok(mut acted) = self.acted.lock() {
            acted.push(what.into());
        }
        Ok(())
    }
}

#[async_trait]
impl Desktop for ScriptedDesktop {
    async fn screenshot(&self) -> Result<Vec<u8>> {
        Ok(vec![0x89, b'P', b'N', b'G'])
    }

    async fn move_to(&self, at: Point) -> Result<()> {
        self.act(format!("move_to {} {}", at.x, at.y))
    }

    async fn click(&self, at: Point, button: Button) -> Result<()> {
        self.act(format!("click {} {} {button:?}", at.x, at.y))
    }

    async fn double_click(&self, at: Point, button: Button) -> Result<()> {
        self.act(format!("double_click {} {} {button:?}", at.x, at.y))
    }

    async fn drag(&self, from: Point, to: Point, button: Button) -> Result<()> {
        self.act(format!(
            "drag {} {} {} {} {button:?}",
            from.x, from.y, to.x, to.y
        ))
    }

    async fn type_text(&self, text: &str) -> Result<()> {
        self.act(format!("type_text {text}"))
    }

    async fn key(&self, chord: &str) -> Result<()> {
        self.act(format!("key {chord}"))
    }

    async fn scroll(&self, at: Point, by: Delta) -> Result<()> {
        self.act(format!("scroll {} {} {}", at.x, at.y, by.dy))
    }

    /// The shape a display server that lets no client read the global pointer
    /// has to use: a named gap, not a coordinate nobody measured.
    async fn cursor(&self) -> Result<Point> {
        Err(Error::Unsupported {
            gaps: vec!["cursor"],
        })
    }

    async fn geometry(&self) -> Result<(u32, u32)> {
        Ok((800, 600))
    }

    async fn alive(&self) -> Result<()> {
        Ok(())
    }

    fn control(&self) -> &Arc<ControlGate> {
        &self.control
    }
}

/// A [`DesktopFactory`] that hands out [`ScriptedDesktop`]s and remembers which
/// screens it was asked for.
pub struct ScriptedDriver {
    server: DisplayServer,
    opened: Mutex<Vec<ScreenId>>,
}

impl Default for ScriptedDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptedDriver {
    /// Claims to be Wayland, because X11 is the one the default already
    /// covers: a test that swaps the driver and still reads X11 has proved
    /// nothing.
    pub fn new() -> Self {
        Self {
            server: DisplayServer::Wayland,
            opened: Mutex::new(Vec::new()),
        }
    }

    pub fn saying(mut self, server: DisplayServer) -> Self {
        self.server = server;
        self
    }

    /// Which screens were opened through it, in order.
    pub fn opened(&self) -> Vec<ScreenId> {
        self.opened
            .lock()
            .map(|opened| opened.clone())
            .unwrap_or_default()
    }
}

impl DesktopFactory for ScriptedDriver {
    fn display_server(&self) -> DisplayServer {
        self.server
    }

    fn open(&self, _host: Arc<MachineHost>, screen: ScreenId) -> Arc<dyn Desktop> {
        if let Ok(mut opened) = self.opened.lock() {
            opened.push(screen);
        }
        Arc::new(ScriptedDesktop::new(screen))
    }
}

/// An image contract that shares nothing with the built-in one.
///
/// Every value differs from [`crate::X11Profile`]'s, so a test that swaps
/// the profile fails on any value the code took from the wrong one.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScriptedProfile;

impl ScriptedProfile {
    pub const IMAGE: &'static str = "scripted/desktop:1";
    pub const SCREEN_COMMAND: &'static str = "scripted-screen";
    pub const VIEW_PORT_BASE: u16 = 7100;
    pub const WIDTH_ENV: &'static str = "SCRIPTED_WIDTH";
    pub const HEIGHT_ENV: &'static str = "SCRIPTED_HEIGHT";
}

impl Profile for ScriptedProfile {
    fn name(&self) -> &str {
        "scripted"
    }

    fn image(&self) -> ImageSource {
        ImageSource::Registry(Self::IMAGE.to_string())
    }

    fn ports(&self) -> PortLayout {
        PortLayout {
            view_base: Self::VIEW_PORT_BASE,
            vnc_base: 7200,
            devtools: None,
            devtools_bridge: None,
            max_screens: 2,
        }
    }

    fn default_size(&self) -> (u32, u32) {
        (640, 480)
    }

    fn support_at(&self, width: u32, height: u32) -> DesktopSupport {
        DesktopSupport {
            display: Some(Display {
                width,
                height,
                // Corrected from the driver at launch, so this is deliberately
                // the wrong answer: it proves which of the two is reported.
                server: DisplayServer::X11,
            }),
            input: true,
            max_screens: 2,
            ..DesktopSupport::default()
        }
    }

    fn driver(&self) -> Arc<dyn DesktopFactory> {
        Arc::new(ScriptedDriver::new())
    }

    fn screen_command(
        &self,
        action: ScreenAction,
        screen: ScreenId,
        extra: &[String],
    ) -> Vec<String> {
        CommandScreen::new(Self::SCREEN_COMMAND).command(action, screen, extra)
    }

    fn boot_command(&self) -> Vec<String> {
        vec!["scripted-desktop".to_string()]
    }

    fn launch_env(&self, width: u32, height: u32) -> BTreeMap<String, String> {
        BTreeMap::from([
            (Self::WIDTH_ENV.to_string(), width.to_string()),
            (Self::HEIGHT_ENV.to_string(), height.to_string()),
        ])
    }

    fn geometry_from(&self, environment: &BTreeMap<String, String>) -> Option<(u32, u32)> {
        let width = environment.get(Self::WIDTH_ENV)?.parse().ok()?;
        let height = environment.get(Self::HEIGHT_ENV)?.parse().ok()?;
        Some((width, height))
    }

    fn screen_env(&self, screen: ScreenId) -> BTreeMap<String, String> {
        BTreeMap::from([("SCRIPTED_SCREEN".to_string(), screen.0.to_string())])
    }

    fn viewer_url(&self, at: &crate::Address, _ticket: Option<&crate::Secret>) -> String {
        format!("{}://{}/scripted", at.scheme.as_str(), at.authority())
    }
}

/// A [`ContainerCli`] that records arguments and never starts anything.
pub struct ScriptedCli {
    inner: ScriptedHost,
    program: String,
}

impl Default for ScriptedCli {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptedCli {
    pub fn new() -> Self {
        Self {
            inner: ScriptedHost::new(),
            program: "docker".to_string(),
        }
    }

    pub fn replying(mut self, result: ExecResult) -> Self {
        self.inner = self.inner.replying(result);
        self
    }

    pub fn saying(mut self, stdout: impl Into<String>) -> Self {
        self.inner = self.inner.saying(stdout);
        self
    }

    pub fn failing(mut self, code: i32, stderr: impl Into<String>) -> Self {
        self.inner = self.inner.failing(code, stderr);
        self
    }

    pub fn calls(&self) -> Vec<Vec<String>> {
        self.inner.calls()
    }

    pub fn last(&self) -> Option<Vec<String>> {
        self.inner.last()
    }

    pub fn count(&self) -> usize {
        self.inner.count()
    }
}

#[async_trait]
impl ContainerCli for ScriptedCli {
    async fn run(&self, args: &[String]) -> Result<ExecResult> {
        Ok(self.inner.record(ScreenId(0), args))
    }

    fn program(&self) -> &str {
        &self.program
    }
}

/// A [`MicroVmApi`] that records what it was asked for and creates nothing.
///
/// What the mapping decided — the ports forwarded, the command that brought
/// the screen up — checked without booting a machine.
pub struct ScriptedMicroVm {
    inner: ScriptedHost,
    plans: Mutex<Vec<Plan>>,
    removed: Mutex<Vec<String>>,
    running: Mutex<bool>,
    images: Mutex<Vec<String>>,
}

impl Default for ScriptedMicroVm {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptedMicroVm {
    pub fn new() -> Self {
        Self {
            inner: ScriptedHost::new(),
            plans: Mutex::new(Vec::new()),
            removed: Mutex::new(Vec::new()),
            running: Mutex::new(true),
            // Nothing handed over yet, which is where a hypervisor starts.
            images: Mutex::new(Vec::new()),
        }
    }

    pub fn replying(mut self, result: ExecResult) -> Self {
        self.inner = self.inner.replying(result);
        self
    }

    pub fn saying(mut self, stdout: impl Into<String>) -> Self {
        self.inner = self.inner.saying(stdout);
        self
    }

    pub fn failing(mut self, code: i32, stderr: impl Into<String>) -> Self {
        self.inner = self.inner.failing(code, stderr);
        self
    }

    /// Say this image has already been handed over.
    pub fn holding(self, image: impl Into<String>) -> Self {
        if let Ok(mut images) = self.images.lock() {
            images.push(image.into());
        }
        self
    }

    /// Answer [`MicroVmApi::running`] with this.
    pub fn stopped(self) -> Self {
        if let Ok(mut running) = self.running.lock() {
            *running = false;
        }
        self
    }

    /// What was asked for, in order.
    pub fn plans(&self) -> Vec<Plan> {
        self.plans
            .lock()
            .map(|plans| plans.clone())
            .unwrap_or_default()
    }

    pub fn calls(&self) -> Vec<Vec<String>> {
        self.inner.calls()
    }

    pub fn last_line(&self) -> String {
        self.inner.last_line()
    }

    pub fn removed(&self) -> Vec<String> {
        self.removed
            .lock()
            .map(|removed| removed.clone())
            .unwrap_or_default()
    }
}

#[async_trait]
impl MicroVmApi for ScriptedMicroVm {
    async fn available(&self) -> Result<()> {
        Ok(())
    }

    async fn has_image(&self, image: &str) -> Result<bool> {
        Ok(self
            .images
            .lock()
            .map(|images| images.iter().any(|held| held == image))
            .unwrap_or(false))
    }

    async fn create(&self, plan: &Plan) -> Result<()> {
        if let Ok(mut plans) = self.plans.lock() {
            plans.push(plan.clone());
        }
        Ok(())
    }

    async fn running(&self, _name: &str) -> Result<bool> {
        Ok(self.running.lock().map(|running| *running).unwrap_or(false))
    }

    async fn remove(&self, name: &str) -> Result<()> {
        if let Ok(mut removed) = self.removed.lock() {
            removed.push(name.to_string());
        }
        Ok(())
    }

    async fn exec(
        &self,
        _name: &str,
        argv: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<ExecResult> {
        // The screen travels in the environment rather than the argument
        // list, so it is recorded where a test can see it.
        let mut recorded = argv.to_vec();
        if let Some(display) = env.get("DISPLAY") {
            recorded.push(format!("DISPLAY={display}"));
        }
        Ok(self.inner.record(ScreenId(0), &recorded))
    }

    async fn read(&self, _name: &str, path: &str) -> Result<Vec<u8>> {
        Ok(path.as_bytes().to_vec())
    }

    async fn write(&self, _name: &str, path: &str, bytes: &[u8]) -> Result<()> {
        let mut recorded = vec!["write".to_string(), path.to_string()];
        recorded.push(bytes.len().to_string());
        self.inner.record(ScreenId(0), &recorded);
        Ok(())
    }
}

/// An [`E2bApi`] that records what was asked and answers from a script.
///
/// A sandbox is a network call away, so a wrong argument list would otherwise
/// cost an account, a template and a boot to find.
pub struct ScriptedE2b {
    inner: ScriptedHost,
    plans: Mutex<Vec<SandboxPlan>>,
    /// Name to sandbox, for both what was created here and what was said to
    /// exist already.
    known: Mutex<BTreeMap<String, Sandbox>>,
    /// Sandbox ID to its metadata, which is what a sweep reads.
    metadata: Mutex<BTreeMap<String, BTreeMap<String, String>>>,
    commands: Mutex<Vec<Vec<String>>>,
    killed: Mutex<Vec<String>>,
    refreshed: Mutex<Vec<String>>,
    found: Mutex<Vec<String>>,
    next: AtomicU64,
}

impl Default for ScriptedE2b {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptedE2b {
    pub fn new() -> Self {
        Self {
            inner: ScriptedHost::new(),
            plans: Mutex::new(Vec::new()),
            known: Mutex::new(BTreeMap::new()),
            metadata: Mutex::new(BTreeMap::new()),
            commands: Mutex::new(Vec::new()),
            killed: Mutex::new(Vec::new()),
            refreshed: Mutex::new(Vec::new()),
            found: Mutex::new(Vec::new()),
            next: AtomicU64::new(0),
        }
    }

    pub fn replying(mut self, result: ExecResult) -> Self {
        self.inner = self.inner.replying(result);
        self
    }

    pub fn saying(mut self, stdout: impl Into<String>) -> Self {
        self.inner = self.inner.saying(stdout);
        self
    }

    pub fn failing(mut self, code: i32, stderr: impl Into<String>) -> Self {
        self.inner = self.inner.failing(code, stderr);
        self
    }

    /// Say a sandbox of this name is already running, as one left by another
    /// process would be.
    pub fn holding(self, name: impl Into<String>, id: impl Into<String>) -> Self {
        let name = name.into();
        let sandbox = Sandbox {
            envd_token: Some("envd".to_string()),
            traffic_token: Some("traffic".to_string()),
            ..Sandbox::new(id)
        };

        if let Ok(mut known) = self.known.lock() {
            known.insert(name.clone(), sandbox.clone());
        }
        if let Ok(mut metadata) = self.metadata.lock() {
            metadata.insert(
                sandbox.id.clone(),
                BTreeMap::from([(e2b::api::NAME_KEY.to_string(), name)]),
            );
        }
        self
    }

    /// What was asked for, in order.
    pub fn plans(&self) -> Vec<SandboxPlan> {
        self.plans
            .lock()
            .map(|plans| plans.clone())
            .unwrap_or_default()
    }

    /// Every command run in a sandbox, in order.
    pub fn commands(&self) -> Vec<Vec<String>> {
        self.commands
            .lock()
            .map(|commands| commands.clone())
            .unwrap_or_default()
    }

    pub fn killed(&self) -> Vec<String> {
        self.killed
            .lock()
            .map(|ids| ids.clone())
            .unwrap_or_default()
    }

    /// Every deadline pushed out, which is what a lazy refresh is measured by.
    pub fn refreshes(&self) -> Vec<String> {
        self.refreshed
            .lock()
            .map(|ids| ids.clone())
            .unwrap_or_default()
    }

    /// Every name looked up through the control plane.
    pub fn found(&self) -> Vec<String> {
        self.found
            .lock()
            .map(|names| names.clone())
            .unwrap_or_default()
    }
}

#[async_trait]
impl E2bApi for ScriptedE2b {
    async fn available(&self) -> Result<()> {
        Ok(())
    }

    async fn create(&self, plan: &SandboxPlan) -> Result<Sandbox> {
        let sandbox = Sandbox {
            envd_token: Some("envd".to_string()),
            traffic_token: Some("traffic".to_string()),
            ..Sandbox::new(format!("sbx-{}", self.next.fetch_add(1, Ordering::Relaxed)))
        };

        let mut metadata = plan.metadata.clone();
        metadata.insert(e2b::api::NAME_KEY.to_string(), plan.name.clone());

        if let Ok(mut plans) = self.plans.lock() {
            plans.push(plan.clone());
        }
        if let Ok(mut known) = self.known.lock() {
            known.insert(plan.name.clone(), sandbox.clone());
        }
        if let Ok(mut all) = self.metadata.lock() {
            all.insert(sandbox.id.clone(), metadata);
        }
        Ok(sandbox)
    }

    async fn find(&self, name: &str) -> Result<Option<Sandbox>> {
        if let Ok(mut found) = self.found.lock() {
            found.push(name.to_string());
        }
        Ok(self
            .known
            .lock()
            .ok()
            .and_then(|known| known.get(name).cloned()))
    }

    async fn kill(&self, id: &str) -> Result<()> {
        if let Ok(mut killed) = self.killed.lock() {
            killed.push(id.to_string());
        }
        Ok(())
    }

    async fn keep_alive(&self, id: &str, _ttl: Duration) -> Result<()> {
        if let Ok(mut refreshed) = self.refreshed.lock() {
            refreshed.push(id.to_string());
        }
        Ok(())
    }

    async fn logs(&self, _id: &str) -> Result<String> {
        Ok(String::new())
    }

    async fn carrying(&self, key: &str) -> Result<Vec<(String, String)>> {
        Ok(self
            .metadata
            .lock()
            .map(|all| {
                all.iter()
                    .filter_map(|(id, metadata)| Some((id.clone(), metadata.get(key)?.clone())))
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn exec(
        &self,
        _sandbox: &Sandbox,
        argv: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<ExecResult> {
        if let Ok(mut commands) = self.commands.lock() {
            commands.push(argv.to_vec());
        }

        // The screen travels in the environment, so it is recorded where a
        // test can see it.
        let mut recorded = argv.to_vec();
        if let Some(display) = env.get("DISPLAY") {
            recorded.push(format!("DISPLAY={display}"));
        }
        Ok(self.inner.record(ScreenId(0), &recorded))
    }

    async fn read(&self, _sandbox: &Sandbox, path: &str) -> Result<Vec<u8>> {
        Ok(path.as_bytes().to_vec())
    }

    async fn write(&self, _sandbox: &Sandbox, path: &str, bytes: &[u8]) -> Result<()> {
        self.inner.record(
            ScreenId(0),
            &[
                "write".to_string(),
                path.to_string(),
                bytes.len().to_string(),
            ],
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_a_scripted_host_answers_in_the_order_it_was_given() {
        let host = ScriptedHost::new().saying("first").saying("second");

        let one = host.run(&["a".to_string()], ScreenId(0)).await.unwrap();
        let two = host.run(&["b".to_string()], ScreenId(1)).await.unwrap();
        let three = host.run(&["c".to_string()], ScreenId(0)).await.unwrap();

        assert_eq!(one.stdout_utf8(), "first");
        assert_eq!(two.stdout_utf8(), "second");
        assert_eq!(
            three.stdout_utf8(),
            "",
            "the queue ran out, so the fallback"
        );
    }

    #[tokio::test]
    async fn test_every_command_and_its_screen_are_recorded() {
        let host = ScriptedHost::new();
        host.run(&["xdotool".to_string()], ScreenId(3))
            .await
            .unwrap();

        assert_eq!(host.last_line(), "xdotool");
        assert_eq!(host.screens(), vec![ScreenId(3)]);
    }
}
