//! microsandbox through its library.
//!
//! Behind `--features microsandbox`, because the crate pulls a hypervisor, an
//! ORM and a database driver behind it and most builds run containers. The
//! same machine as [`super::msb`], reached without shelling out.

use crate::error::{Error, Result};
use crate::exec::ExecResult;
use crate::microvm::{MicroVm, MicroVmApi, Plan};
use ::microsandbox::{MicrosandboxError, Sandbox};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Anything the vendor reports, as one of ours.
///
/// Coarse on purpose: the only question a caller has is whether to retry.
fn from_vendor(error: MicrosandboxError) -> Error {
    Error::transport(error.to_string(), false)
}

/// A handle names a machine; a [`Sandbox`] can be talked to.
///
/// `connect` rather than `start`: the machine is already running.
async fn connect(name: &str) -> Result<Sandbox> {
    Sandbox::get(name)
        .await
        .map_err(from_vendor)?
        .connect()
        .await
        .map_err(from_vendor)
}

/// microsandbox on this host.
#[derive(Debug, Default, Clone, Copy)]
pub struct Microsandbox;

#[async_trait]
impl MicroVmApi for Microsandbox {
    async fn available(&self) -> Result<()> {
        // Nothing to ask a hypervisor that holds no machines. A failed
        // create is where it reports itself.
        Ok(())
    }

    async fn create(&self, plan: &Plan) -> Result<()> {
        let mut builder = Sandbox::builder(plan.name.clone()).image(plan.image.clone());

        if let Some(cpus) = plan.cpus {
            builder = builder.cpus(cpus);
        }
        if let Some(mib) = plan.memory_mib {
            builder = builder.memory(mib as u32);
        }
        if !plan.network {
            // Removes the interface entirely, which is a stronger "off"
            // than a container's empty network namespace.
            builder = builder.disable_network();
        }
        for (host, guest) in &plan.ports {
            // Loopback by default in this builder. The screen has no
            // password on it.
            builder = builder.port(*host, *guest);
        }
        for (key, value) in &plan.env {
            builder = builder.env(key.clone(), value.clone());
        }
        if plan.replace {
            builder = builder.replace();
        }

        // Detached, or the machine dies with the handle. `create`
        // returns a live sandbox and dropping it tears the microVM down,
        // so the next exec reconnects to nothing. A place has to outlive
        // the call that made it.
        builder
            .create_detached()
            .await
            .map(|_| ())
            .map_err(from_vendor)
    }

    async fn running(&self, name: &str) -> Result<bool> {
        Ok(Sandbox::get(name).await.is_ok())
    }

    async fn remove(&self, name: &str) -> Result<()> {
        // Stopped first: remove refuses a running machine, and a failed
        // disposal leaves a microVM holding its whole memory ceiling until
        // somebody finds it by hand.
        if let Ok(sandbox) = connect(name).await {
            let _ = sandbox.stop().await;
        }
        Sandbox::remove(name).await.map_err(from_vendor)
    }

    async fn exec(
        &self,
        name: &str,
        argv: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<ExecResult> {
        let Some((command, arguments)) = argv.split_first() else {
            return Err(Error::denied("an empty command has nothing to run"));
        };

        let sandbox = connect(name).await?;
        let arguments = arguments.to_vec();
        let env = env.clone();

        // `exec_with` rather than `exec`: the plain form takes only a
        // command and arguments, so the display would be dropped without
        // anything saying so — and every screenshot would come back from
        // whichever display the machine happened to default to.
        let output = sandbox
            .exec_with(command, move |mut options| {
                options = options.args(arguments);
                for (key, value) in env {
                    options = options.env(key, value);
                }
                options
            })
            .await
            .map_err(from_vendor)?;

        Ok(ExecResult {
            code: output.status().code,
            stdout: output.stdout_bytes().to_vec(),
            stderr: output.stderr_bytes().to_vec(),
            timed_out: false,
        })
    }

    async fn read(&self, name: &str, path: &str) -> Result<Vec<u8>> {
        let sandbox = connect(name).await?;
        let bytes = sandbox.fs().read(path).await.map_err(from_vendor)?;
        Ok(bytes.to_vec())
    }

    async fn write(&self, name: &str, path: &str, bytes: &[u8]) -> Result<()> {
        let sandbox = connect(name).await?;
        sandbox.fs().write(path, bytes).await.map_err(from_vendor)
    }
}

/// A hypervisor-backed machine, ready to hand to
/// [`Computer::builder`](crate::Computer::builder).
pub fn machine() -> MicroVm {
    MicroVm::new(Arc::new(Microsandbox)).named("microsandbox")
}
