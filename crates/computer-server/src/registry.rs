//! The boxes this server is holding, and the lock that keeps a batch whole.

use crate::error::{ApiError, ApiResult};
use computer::{Computer, ScreenId};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::{Mutex, RwLock};

pub struct Entry {
    pub id: String,
    pub spec_digest: String,
    pub created_at: SystemTime,
    pub screens: u32,
    pub width: u32,
    pub height: u32,
    pub computer: Computer,
    /// A batch must not interleave with another caller's: a click landing
    /// between somebody else's move and click goes to the wrong place, and the
    /// frame that comes back looks like it worked.
    ///
    /// Per screen rather than per box, so two agents on two screens of one box
    /// do not queue behind each other.
    locks: Mutex<BTreeMap<u32, Arc<Mutex<()>>>>,
    /// So `have_frame` is answerable without re-encoding a picture nobody
    /// wants.
    frames: Mutex<BTreeMap<u32, String>>,
}

impl Entry {
    pub async fn screen_lock(&self, screen: u32) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock().await;
        Arc::clone(locks.entry(screen).or_default())
    }

    pub async fn remember_frame(&self, screen: u32, hash: &str) {
        self.frames.lock().await.insert(screen, hash.to_string());
    }

    /// Screen 0 is the box's own, already started. The rest start on first
    /// ask, and are taken unfenced because this server is the only thing
    /// holding them — its own lock is the serialisation, not a lease.
    pub async fn desktop(&self, screen: u32) -> ApiResult<Box<dyn AsDesktop + Send + '_>> {
        if screen >= self.screens {
            return Err(ApiError::not_found(format!(
                "this box has {} screen(s) and screen {screen} is not one of them",
                self.screens
            )));
        }

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
        spec_digest: String,
        screens: u32,
        width: u32,
        height: u32,
        computer: Computer,
    ) -> Arc<Entry> {
        let entry = Arc::new(Entry {
            id: id.clone(),
            spec_digest,
            created_at: SystemTime::now(),
            screens,
            width,
            height,
            computer,
            locks: Mutex::new(BTreeMap::new()),
            frames: Mutex::new(BTreeMap::new()),
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

    /// Through the machine rather than `Computer::shutdown`, which needs the
    /// handle by value while other requests may still be holding one.
    pub async fn remove(&self, id: &str) -> ApiResult<()> {
        let entry = self
            .boxes
            .write()
            .await
            .remove(id)
            .ok_or_else(|| ApiError::not_found(format!("no box {id}")))?;

        entry.computer.machine().stop(&entry.id).await?;
        Ok(())
    }

    pub async fn last_frame(&self, id: &str, screen: u32) -> Option<String> {
        let entry = self.boxes.read().await.get(id).cloned()?;
        let frames = entry.frames.lock().await;
        frames.get(&screen).cloned()
    }
}
