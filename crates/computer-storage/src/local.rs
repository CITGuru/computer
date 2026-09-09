//! A directory on this host, addressed by key.

use crate::{Blobs, Error, Result};
use async_trait::async_trait;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct LocalDir {
    root: PathBuf,
    writes: AtomicU64,
}

impl LocalDir {
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            writes: AtomicU64::new(0),
        }
    }

    /// Where a key lives, refused if it could reach outside the root.
    fn path(&self, key: &str) -> Result<PathBuf> {
        let mut path = self.root.clone();

        for part in key.split('/') {
            let bad = part.is_empty() || part == "." || part == "..";
            if bad {
                return Err(Error::Internal(format!("{key:?} is not usable as a key")));
            }
            path.push(part);
        }

        Ok(path)
    }
}

#[async_trait]
impl Blobs for LocalDir {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        match tokio::fs::read(self.path(key)?).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(unreachable(key, error)),
        }
    }

    async fn put(&self, key: &str, bytes: &[u8]) -> Result<()> {
        let path = self.path(key)?;

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| unreachable(key, error))?;
        }

        let mine = self.writes.fetch_add(1, Ordering::Relaxed);
        let partial = path.with_extension(format!("partial-{mine}"));

        tokio::fs::write(&partial, bytes)
            .await
            .map_err(|error| unreachable(key, error))?;

        if let Err(error) = tokio::fs::rename(&partial, &path).await {
            tokio::fs::remove_file(&partial).await.ok();
            return Err(unreachable(key, error));
        }

        Ok(())
    }

    async fn list(&self, prefix: &str, start_after: Option<&str>) -> Result<Vec<String>> {
        let (under, _) = prefix.rsplit_once('/').unwrap_or(("", prefix));
        let from = match under.is_empty() {
            true => self.root.clone(),
            false => self.path(under)?,
        };

        let mut keys = Vec::new();
        let mut pending = vec![(from, under.to_string())];

        while let Some((directory, at)) = pending.pop() {
            let mut entries = match tokio::fs::read_dir(&directory).await {
                Ok(entries) => entries,
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(error) => return Err(unreachable(prefix, error)),
            };

            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|error| unreachable(prefix, error))?
            {
                let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                    continue;
                };

                let key = match at.is_empty() {
                    true => name,
                    false => format!("{at}/{name}"),
                };

                let kind = entry
                    .file_type()
                    .await
                    .map_err(|error| unreachable(prefix, error))?;

                match kind.is_dir() {
                    true => pending.push((entry.path(), key)),
                    false => keys.push(key),
                }
            }
        }

        keys.retain(|key| {
            key.starts_with(prefix) && start_after.is_none_or(|after| key.as_str() > after)
        });
        keys.sort();

        Ok(keys)
    }

    async fn delete_prefix(&self, prefix: &str) -> Result<()> {
        for key in self.list(prefix, None).await? {
            let path = self.path(&key)?;

            match tokio::fs::remove_file(&path).await {
                Ok(()) => (),
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(error) => return Err(unreachable(&key, error)),
            }

            prune(&self.root, path.parent()).await;
        }

        Ok(())
    }
}

async fn prune(root: &Path, mut from: Option<&Path>) {
    while let Some(directory) = from {
        if directory == root || !directory.starts_with(root) {
            return;
        }
        if tokio::fs::remove_dir(directory).await.is_err() {
            return;
        }
        from = directory.parent();
    }
}

fn unreachable(key: &str, error: std::io::Error) -> Error {
    Error::Unavailable(format!("{key}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conformance;
    use crate::files::Files;
    use std::sync::atomic::AtomicU32;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A directory of its own per test, taken away afterwards.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);

            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|since| since.as_nanos())
                .unwrap_or_default();
            let mine = NEXT.fetch_add(1, Ordering::Relaxed);

            let path = std::env::temp_dir().join(format!("computer-storage-{nanos}-{mine}"));
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[tokio::test]
    async fn test_a_directory_behaves_like_a_blob_backend() {
        let scratch = Scratch::new();
        conformance::blobs(&LocalDir::at(&scratch.0)).await;
    }

    #[tokio::test]
    async fn test_a_store_on_a_directory_behaves_like_a_store() {
        let scratch = Scratch::new();
        conformance::store(&Files::over(LocalDir::at(&scratch.0))).await;
    }

    #[tokio::test]
    async fn test_frames_on_a_directory_behave_like_a_frame_store() {
        let scratch = Scratch::new();
        conformance::frames(&Files::over(LocalDir::at(&scratch.0))).await;
    }

    #[tokio::test]
    async fn test_a_store_on_a_directory_prunes() {
        let scratch = Scratch::new();
        let store = Files::over(LocalDir::at(&scratch.0));

        conformance::pruning(&store).await;
        conformance::dropping(&store).await;
    }

    #[tokio::test]
    async fn test_a_key_cannot_climb_out_of_the_root() {
        let scratch = Scratch::new();
        let local = LocalDir::at(&scratch.0);

        assert!(local.get("../outside").await.is_err());
        assert!(local.put("a/../../outside", b"no").await.is_err());
        assert!(local.get("/etc/passwd").await.is_err());
    }

    #[tokio::test]
    async fn test_an_emptied_prefix_leaves_no_directories_behind() {
        let scratch = Scratch::new();
        let local = LocalDir::at(&scratch.0);

        local
            .put("boxes/box_1/traces/000000000000-000000000000.jsonl", b"{}")
            .await
            .expect("written");
        local.delete_prefix("boxes/box_1/").await.expect("deleted");

        assert!(
            !scratch.0.join("boxes/box_1").exists(),
            "a box that has been forgotten leaves no tree behind"
        );
        assert!(
            scratch.0.exists(),
            "and the root it was handed is left alone"
        );
    }

    #[tokio::test]
    async fn test_a_partial_write_is_never_a_key() {
        let scratch = Scratch::new();
        let local = LocalDir::at(&scratch.0);

        local
            .put("boxes/box_1/main.json", b"{\"id\":\"box_1\"}")
            .await
            .expect("written");

        assert_eq!(
            local.list("boxes/", None).await.expect("listed"),
            vec!["boxes/box_1/main.json".to_string()],
            "the temporary file did not survive the rename"
        );
    }
}
