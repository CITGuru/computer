use crate::error::{ApiError, ApiResult};
use computer::spec::Resolved;
use computer::{Computer, ScreenId};
use computer_types::Spec;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::{Mutex, RwLock};

pub struct Entry {
    pub id: String,
    pub runtime: String,
    pub spec: Spec,
    pub created_at: SystemTime,
    pub screens: u32,
    pub width: u32,
    pub height: u32,
    pub computer: Computer,
    /// Per screen, so a batch never interleaves with another caller's on that screen.
    locks: Mutex<BTreeMap<u32, Arc<Mutex<()>>>>,
}

impl Entry {
    pub fn spec_digest(&self) -> String {
        self.spec.digest()
    }

    /// Refuses unknown screens: nothing prunes the lock map, so any number would grow it.
    pub async fn screen_lock(&self, screen: u32) -> ApiResult<Arc<Mutex<()>>> {
        self.check(screen)?;

        let mut locks = self.locks.lock().await;
        Ok(Arc::clone(locks.entry(screen).or_default()))
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
    fn as_desktop(&self) -> &dyn computer::Desktop;
    fn as_screen(&self) -> Option<&computer::Screen>;
}

struct Primary<'a>(&'a Computer);

impl AsDesktop for Primary<'_> {
    fn as_desktop(&self) -> &dyn computer::Desktop {
        self.0.primary()
    }

    fn as_screen(&self) -> Option<&computer::Screen> {
        Some(self.0.primary())
    }
}

struct Held(computer::Screen);

impl AsDesktop for Held {
    fn as_desktop(&self) -> &dyn computer::Desktop {
        &self.0
    }

    fn as_screen(&self) -> Option<&computer::Screen> {
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
    ) -> Arc<Entry> {
        let entry = Arc::new(Entry {
            id: id.clone(),
            runtime,
            spec,
            created_at: SystemTime::now(),
            screens: size.screens,
            width: size.width,
            height: size.height,
            computer,
            locks: Mutex::new(BTreeMap::new()),
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
            created_at: was.created_at,
            screens: was.screens,
            width: was.width,
            height: was.height,
            computer,
            locks: Mutex::new(BTreeMap::new()),
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
