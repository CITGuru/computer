use crate::error::{Error, Result};
use crate::exec::ExecResult;
use crate::microvm::{MicroVm, MicroVmApi, Plan};
use ::microsandbox::{MicrosandboxError, Sandbox};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::Arc;

fn from_vendor(error: MicrosandboxError) -> Error {
    Error::transport(error.to_string(), false)
}

async fn connect(name: &str) -> Result<Sandbox> {
    Sandbox::get(name)
        .await
        .map_err(from_vendor)?
        .connect()
        .await
        .map_err(from_vendor)
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Microsandbox;

#[async_trait]
impl MicroVmApi for Microsandbox {
    async fn available(&self) -> Result<()> {
        // The hypervisor only reports itself through a failed create.
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
            builder = builder.disable_network();
        }
        for (host, guest) in &plan.ports {
            builder = builder.port(*host, *guest);
        }
        for (key, value) in &plan.env {
            builder = builder.env(key.clone(), value.clone());
        }
        if plan.replace {
            builder = builder.replace();
        }

        // Dropping the sandbox `create` returns tears the microVM down.
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
        // Remove refuses a running machine.
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

        // Plain `exec` silently drops env, and with it the display.
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

pub fn machine() -> MicroVm {
    MicroVm::new(Arc::new(Microsandbox)).named("microsandbox")
}
