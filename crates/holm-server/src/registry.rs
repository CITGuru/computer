use crate::error::{ApiError, ApiResult};
use holm::spec::Resolved;
use holm::{Computer, ScreenId};
use holm_types::{Placement, Spec};

pub struct Made {
    pub placement: Placement,
    pub owner: Option<String>,
    pub created_at: Option<SystemTime>,
}
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;

pub struct Entry {
    pub id: String,
    pub runtime: String,
    pub spec: Spec,
    pub placement: Placement,
    pub owner: Option<String>,
    pub created_at: SystemTime,
    pub screens: u32,
    pub width: u32,
    pub height: u32,
    pub computer: Computer,
}

impl Entry {
    pub fn spec_digest(&self) -> String {
        self.spec.digest()
    }

    pub fn screen_lock(&self, screen: u32) -> ApiResult<String> {
        self.check(screen)?;

        Ok(format!("screen/{}/{screen}", self.id))
    }

    fn check(&self, screen: u32) -> ApiResult<()> {
        if screen >= self.screens {
            return Err(ApiError::not_found(format!(
                "this box has {} screen(s) and screen {screen} is not one of them",
                self.screens
            )));
        }

        Ok(())
    }

    /// Extra screens are taken unfenced: this server's lock serialises them, not a lease.
    pub async fn desktop(&self, screen: u32) -> ApiResult<Box<dyn AsDesktop + Send + '_>> {
        self.check(screen)?;

        if screen == 0 {
            return Ok(Box::new(Primary(&self.computer)));
        }

        let held = self.computer.screen_unfenced(ScreenId(screen)).await?;
        Ok(Box::new(Held(held)))
    }
}

pub trait AsDesktop {
    fn as_desktop(&self) -> &dyn holm::Desktop;
    fn as_screen(&self) -> Option<&holm::Screen>;
}

struct Primary<'a>(&'a Computer);

impl AsDesktop for Primary<'_> {
    fn as_desktop(&self) -> &dyn holm::Desktop {
        self.0.primary()
    }

    fn as_screen(&self) -> Option<&holm::Screen> {
        Some(self.0.primary())
    }
}

struct Held(holm::Screen);

impl AsDesktop for Held {
    fn as_desktop(&self) -> &dyn holm::Desktop {
        &self.0
    }

    fn as_screen(&self) -> Option<&holm::Screen> {
        Some(&self.0)
    }
}

#[derive(Default)]
pub struct Registry {
    boxes: RwLock<BTreeMap<String, Arc<Entry>>>,
}

impl Registry {
    pub async fn insert(
        &self,
        id: String,
        runtime: String,
        spec: Spec,
        size: Resolved,
        computer: Computer,
        made: Made,
    ) -> Arc<Entry> {
        let entry = Arc::new(Entry {
            id: id.clone(),
            runtime,
            spec,
            placement: made.placement,
            owner: made.owner,
            created_at: made.created_at.unwrap_or_else(SystemTime::now),
            screens: size.screens,
            width: size.width,
            height: size.height,
            computer,
        });

        self.boxes.write().await.insert(id, Arc::clone(&entry));
        entry
    }

    pub async fn get(&self, id: &str) -> ApiResult<Arc<Entry>> {
        self.boxes
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| ApiError::not_found(format!("no box {id}")))
    }

    pub async fn list(&self) -> Vec<Arc<Entry>> {
        self.boxes.read().await.values().cloned().collect()
    }

    pub async fn replace(&self, id: &str, computer: Computer) -> ApiResult<Arc<Entry>> {
        let mut boxes = self.boxes.write().await;

        let was = boxes
            .get(id)
            .ok_or_else(|| ApiError::not_found(format!("no box {id}")))?;

        let entry = Arc::new(Entry {
            id: was.id.clone(),
            runtime: was.runtime.clone(),
            spec: was.spec.clone(),
            placement: was.placement.clone(),
            owner: was.owner.clone(),
            created_at: was.created_at,
            screens: was.screens,
            width: was.width,
            height: was.height,
            computer,
        });

        boxes.insert(id.to_string(), Arc::clone(&entry));
        Ok(entry)
    }

    pub async fn forget(&self, id: &str) {
        self.boxes.write().await.remove(id);
    }

    /// Not `Computer::shutdown`: it needs the handle by value, and requests may hold one.
    pub async fn remove(&self, id: &str) -> ApiResult<()> {
        let entry = self
            .boxes
            .write()
            .await
            .remove(id)
            .ok_or_else(|| ApiError::not_found(format!("no box {id}")))?;

        entry.computer.machine().remove(&entry.id).await?;
        Ok(())
    }
}
