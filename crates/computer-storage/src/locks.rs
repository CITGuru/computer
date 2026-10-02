use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::OwnedMutexGuard;

pub struct Lock(#[allow(dead_code)] Box<dyn Send + Sync>);

impl Lock {
    pub fn of(held: impl Send + Sync + 'static) -> Self {
        Self(Box::new(held))
    }
}

#[derive(Default)]
pub struct Locks(Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>);

impl Locks {
    pub async fn hold(&self, name: &str) -> OwnedMutexGuard<()> {
        let lock = {
            let mut held = self.0.lock().unwrap_or_else(|broken| broken.into_inner());
            held.retain(|_, lock| Arc::strong_count(lock) > 1);
            Arc::clone(held.entry(name.to_string()).or_default())
        };

        lock.lock_owned().await
    }
}
