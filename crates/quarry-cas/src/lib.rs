use quarry_core::{GcReport, QuarryError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tempfile::NamedTempFile;

const BLAKE3_HASH_HEX_LEN: usize = 64;

#[derive(Clone, Debug)]
pub struct DiskCas {
    root: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Blake3Hash(String);

impl Blake3Hash {
    fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Blake3Hash {
    type Err = QuarryError;

    fn from_str(value: &str) -> Result<Self> {
        if value.len() != BLAKE3_HASH_HEX_LEN || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(QuarryError::Invariant(format!(
                "invalid BLAKE3 hash {value}"
            )));
        }
        Ok(Self(value.to_ascii_lowercase()))
    }
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
        self.object_path_for_hash(&hash.parse()?)
    }

    fn object_path_for_hash(&self, hash: &Blake3Hash) -> Result<PathBuf> {
        Ok(self
            .root
            .join("objects")
            .join(&hash.as_str()[0..2])
            .join(&hash.as_str()[2..]))
    }

    pub fn put(&self, bytes: &[u8]) -> Result<BlobInfo> {
        let hash = Self::hash(bytes);
        let hash = hash.parse::<Blake3Hash>()?;
        let path = self.object_path_for_hash(&hash)?;
        if path.exists() {
            return Ok(BlobInfo {
                hash: hash.as_str().to_string(),
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
            hash: hash.as_str().to_string(),
            byte_size: bytes.len() as u64,
            path,
        })
    }

    pub fn read(&self, hash: &str) -> Result<Vec<u8>> {
        let hash = hash.parse::<Blake3Hash>()?;
        let path = self.object_path_for_hash(&hash)?;
        if !path.exists() {
            return Err(QuarryError::NotFound(format!("blob {}", hash.as_str())));
        }
        Ok(fs::read(path)?)
    }

    pub fn exists(&self, hash: &str) -> Result<bool> {
        let hash = hash.parse::<Blake3Hash>()?;
        Ok(self.object_path_for_hash(&hash)?.exists())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_path_rejects_short_hex_hashes() {
        let root = tempfile::tempdir().unwrap();
        let cas = DiskCas::open(root.path()).unwrap();

        let error = cas.object_path("abcd").unwrap_err();

        assert!(error.to_string().contains("invalid BLAKE3 hash abcd"));
    }
}
