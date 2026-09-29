pub mod dockerfile;

use crate::bundle::Bundle;
use crate::error::{Error, Result};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    pub path: String,
    pub bytes: Vec<u8>,
    pub mode: u32,
}

impl File {
    pub fn new(path: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            path: path.into(),
            bytes,
            mode: 0o644,
        }
    }
}

pub fn bundled(bundle: &Bundle) -> Vec<File> {
    bundle
        .files
        .iter()
        .map(|(name, body)| File::new(*name, body.as_bytes().to_vec()))
        .collect()
}

pub fn directory(root: &Path) -> Result<Vec<File>> {
    let mut files = Vec::new();
    walk(root, root, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn walk(root: &Path, directory: &Path, files: &mut Vec<File>) -> Result<()> {
    let failed = |path: &Path, error: std::io::Error| {
        Error::denied(format!("image directory {}: {error}", path.display()))
    };

    for entry in std::fs::read_dir(directory).map_err(|error| failed(directory, error))? {
        let path = entry.map_err(|error| failed(directory, error))?.path();
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| failed(&path, error))?;

        if metadata.is_dir() {
            walk(root, &path, files)?;
            continue;
        }

        if !metadata.is_file() {
            return Err(Error::Unsupported {
                gaps: vec!["a symlink in an image directory built at a vendor"],
            });
        }

        let relative = path.strip_prefix(root).unwrap_or(&path);
        #[cfg(unix)]
        let mode = std::os::unix::fs::PermissionsExt::mode(&metadata.permissions()) & 0o777;
        #[cfg(not(unix))]
        let mode = 0o644;

        files.push(File {
            path: relative.display().to_string(),
            bytes: std::fs::read(&path).map_err(|error| failed(&path, error))?,
            mode,
        });
    }

    Ok(())
}
