//! cargo run --example custom_sandbox

use async_trait::async_trait;
use computer::sandboxes::remote::{self, NAME_KEY, RemoteApi, Sandbox, SandboxPlan};
use computer::{
    Auth, Computer, ContainerCli, DockerMachine, Error, ExecResult, Machine, Result, SystemDocker,
    X11Profile,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

struct Fleet {
    docker: Arc<SystemDocker>,
}

impl Fleet {
    fn new() -> Self {
        Self {
            docker: Arc::new(SystemDocker::default()),
        }
    }

    async fn run(&self, args: &[&str]) -> Result<ExecResult> {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        self.docker.run(&args).await
    }

    async fn control(&self, args: &[&str]) -> Result<String> {
        let result = self.run(args).await?;

        match result.ok() {
            true => Ok(result.stdout_utf8().trim().to_string()),
            false => Err(Error::Unavailable {
                runtime: "fleet".to_string(),
                detail: result.stderr_utf8().trim().to_string(),
            }),
        }
    }

    /// A vendor publishes ports at its own addresses; the viewer URL is built from these.
    async fn endpoints(&self, id: &str) -> Result<BTreeMap<u16, String>> {
        let printed = self.control(&["port", id]).await?;

        Ok(computer::runtime::parse_ports(&printed)
            .into_iter()
            .map(|(inside, outside)| (inside, format!("http://127.0.0.1:{outside}")))
            .collect())
    }

    async fn sandbox(&self, id: &str) -> Result<Sandbox> {
        Ok(Sandbox {
            id: id.to_string(),
            endpoints: self.endpoints(id).await?,
            token: None,
        })
    }
}

#[async_trait]
impl RemoteApi for Fleet {
    fn vendor(&self) -> &str {
        "fleet"
    }

    async fn available(&self) -> Result<()> {
        self.control(&["version", "--format", "{{.Server.Version}}"])
            .await
            .map(|_| ())
    }

    async fn create(&self, plan: &SandboxPlan) -> Result<Sandbox> {
        let mut args = vec![
            "run".to_string(),
            "--detach".to_string(),
            // `RemoteMachine` boots the sandbox, so it must not start a screen of its own.
            "--entrypoint".to_string(),
            "sleep".to_string(),
        ];

        for (key, value) in &plan.metadata {
            args.push("--label".to_string());
            args.push(format!("{key}={value}"));
        }
        for (key, value) in &plan.env {
            args.push("--env".to_string());
            args.push(format!("{key}={value}"));
        }
        for port in &plan.publish {
            args.push("--publish".to_string());
            args.push(format!("127.0.0.1::{port}"));
        }
        if !plan.network {
            args.push("--network".to_string());
            args.push("none".to_string());
        }

        args.push(plan.image.clone());
        args.push("infinity".to_string());

        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        let id = self.control(&borrowed).await?;

        self.sandbox(&id).await
    }

    /// A vendor assigns its own ID, so the name travels as metadata.
    async fn find(&self, name: &str) -> Result<Option<Sandbox>> {
        let filter = format!("label={NAME_KEY}={name}");
        let found = self
            .control(&["ps", "--quiet", "--filter", &filter])
            .await?;

        match found.lines().next() {
            Some(id) => Ok(Some(self.sandbox(id).await?)),
            None => Ok(None),
        }
    }

    async fn kill(&self, id: &str) -> Result<()> {
        self.control(&["rm", "--force", "--volumes", id])
            .await
            .map(|_| ())
    }

    async fn exec(
        &self,
        sandbox: &Sandbox,
        argv: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<ExecResult> {
        let mut args = vec!["exec".to_string()];

        // A vendor that drops this runs every click against display zero.
        for (key, value) in env {
            args.push("--env".to_string());
            args.push(format!("{key}={value}"));
        }

        args.push(sandbox.id.clone());
        args.extend(argv.iter().cloned());

        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run(&borrowed).await
    }

    async fn read(&self, sandbox: &Sandbox, path: &str) -> Result<Vec<u8>> {
        let local = std::env::temp_dir().join(format!("fleet-{}", uid()));
        self.control(&[
            "cp",
            &format!("{}:{path}", sandbox.id),
            &local.display().to_string(),
        ])
        .await?;

        let bytes = std::fs::read(&local).map_err(|error| Error::denied(error.to_string()));
        let _ = std::fs::remove_file(&local);
        bytes
    }

    async fn write(&self, sandbox: &Sandbox, path: &str, bytes: &[u8]) -> Result<()> {
        let local = std::env::temp_dir().join(format!("fleet-{}", uid()));
        std::fs::write(&local, bytes).map_err(|error| Error::denied(error.to_string()))?;

        let sent = self
            .control(&[
                "cp",
                &local.display().to_string(),
                &format!("{}:{path}", sandbox.id),
            ])
            .await;

        let _ = std::fs::remove_file(&local);
        sent.map(|_| ())
    }

    async fn logs(&self, id: &str) -> Result<String> {
        Ok(self.run(&["logs", id]).await?.stdout_utf8())
    }

    /// nobody's to remove.
    async fn carrying(&self, key: &str) -> Result<Vec<(String, String)>> {
        let format = format!("{{{{.ID}}}}\t{{{{.Label \"{key}\"}}}}");
        let listed = self
            .control(&[
                "ps",
                "--filter",
                &format!("label={key}"),
                "--format",
                &format,
            ])
            .await?;

        Ok(listed
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .map(|(id, value)| (id.to_string(), value.to_string()))
            .collect())
    }

    fn sweepable(&self) -> bool {
        true
    }

    /// `Drop` cannot await, so a dropped handle takes the box only through a command.
    fn reaper(&self, id: &str) -> Option<(String, Vec<String>)> {
        Some((
            self.docker.program().to_string(),
            vec!["rm".to_string(), "--force".to_string(), id.to_string()],
        ))
    }

    /// Leave this out where the vendor needs a template built first.
    async fn ensure_image(&self, config: &computer::Config) -> Result<()> {
        DockerMachine::new(Arc::clone(&self.docker) as Arc<dyn ContainerCli>)
            .ensure_image(config)
            .await
    }
}

fn uid() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos() as u64)
        .unwrap_or_default()
}

#[tokio::main]
async fn main() -> Result<()> {
    let (machine, profile) = remote::pair(Arc::new(Fleet::new()), Arc::new(X11Profile));

    println!("starting a sandbox …");
    let computer = Computer::builder()
        .machine(Arc::new(machine.public_viewer(true)))
        .profile(profile)
        // A public viewer is gated the same way a routable bind is.
        .auth(Auth::Password)
        .launch()
        .await?;

    println!("  runtime  {}", computer.runtime());
    if let (Some(url), Some(credentials)) = (computer.viewer_url(), computer.credentials()) {
        println!("  watch it {url}");
        println!("  password {}", credentials.view.expose());
    }

    computer.open_url("https://example.com").await?;
    computer
        .wait_until_still(Duration::from_millis(500), Duration::from_secs(20))
        .await?;

    let frame = computer.screenshot().await?;
    std::fs::write("custom_sandbox.png", &frame).ok();
    println!("  screenshot: {} bytes -> custom_sandbox.png", frame.len());

    computer
        .write_file("/tmp/note.txt", b"through the seam")
        .await?;
    let read_back = computer.read_file("/tmp/note.txt").await?;
    println!("  file: {}", String::from_utf8_lossy(&read_back));

    computer.shutdown().await
}
