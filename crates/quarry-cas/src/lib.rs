use quarry_core::{GcReport, QuarryError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

#[derive(Clone, Debug)]
pub struct DiskCas {
    root: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlobInfo {
    pub hash: String,
    pub byte_size: u64,
    pub path: PathBuf,
}

impl DiskCas {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(root.join("objects"))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn hash(bytes: &[u8]) -> String {
        blake3::hash(bytes).to_hex().to_string()
    }

    pub fn object_path(&self, hash: &str) -> Result<PathBuf> {
        if hash.len() < 4 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(QuarryError::Invariant(format!(
                "invalid BLAKE3 hash {hash}"
            )));
        }
        Ok(self.root.join("objects").join(&hash[0..2]).join(&hash[2..]))
    }

    pub fn put(&self, bytes: &[u8]) -> Result<BlobInfo> {
        let hash = Self::hash(bytes);
        let path = self.object_path(&hash)?;
        if path.exists() {
            return Ok(BlobInfo {
                hash,
                byte_size: bytes.len() as u64,
                path,
            });
        }

        let parent = path
            .parent()
            .ok_or_else(|| QuarryError::Invariant("CAS object path has no parent".to_string()))?;
        fs::create_dir_all(parent)?;

        let mut tmp = NamedTempFile::new_in(parent)?;
        tmp.write_all(bytes)?;
        tmp.as_file().sync_all()?;
        match tmp.persist(&path) {
            Ok(file) => {
                sync_parent(parent)?;
                drop(file);
            }
            Err(err) if path.exists() => {
                drop(err);
            }
            Err(err) => return Err(QuarryError::Io(err.error)),
        }

        Ok(BlobInfo {
            hash,
            byte_size: bytes.len() as u64,
            path,
        })
    }

    pub fn read(&self, hash: &str) -> Result<Vec<u8>> {
        let path = self.object_path(hash)?;
        if !path.exists() {
            return Err(QuarryError::NotFound(format!("blob {hash}")));
        }
        Ok(fs::read(path)?)
    }

    pub fn exists(&self, hash: &str) -> Result<bool> {
        Ok(self.object_path(hash)?.exists())
    }

    pub fn gc<I>(&self, reachable_hashes: I) -> Result<GcReport>
    where
        I: IntoIterator<Item = String>,
    {
        let reachable: HashSet<String> = reachable_hashes.into_iter().collect();
        let mut removed = 0;
        let objects = self.root.join("objects");
        if objects.exists() {
            for shard in fs::read_dir(&objects)? {
                let shard = shard?;
                if !shard.file_type()?.is_dir() {
                    continue;
                }
                let shard_name = shard.file_name().to_string_lossy().to_string();
                for object in fs::read_dir(shard.path())? {
                    let object = object?;
                    if !object.file_type()?.is_file() {
                        continue;
                    }
                    let hash = format!("{}{}", shard_name, object.file_name().to_string_lossy());
                    if !reachable.contains(&hash) {
                        fs::remove_file(object.path())?;
                        removed += 1;
                    }
                }
            }
        }
        Ok(GcReport {
            reachable: reachable.len(),
            removed,
        })
    }
}

fn sync_parent(path: &Path) -> Result<()> {
    let dir = OpenOptions::new().read(true).open(path)?;
    File::sync_all(&dir)?;
    Ok(())
}
