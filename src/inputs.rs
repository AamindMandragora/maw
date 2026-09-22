use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, thiserror::Error)]
pub enum InputsError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{path}: {source}")]
    Json { path: PathBuf, source: serde_json::Error },
}

// mtime and size of a file when it was last hashed
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Stamp {
    mtime: u128,
    size: u64,
    hash: String,
}

// input file -> stamp, so unchanged files are never re-read
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Inputs {
    files: BTreeMap<PathBuf, Stamp>,
}

pub fn hash_bytes(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

// one hash over several hashes, for cache keys
pub fn combine(parts: &[&str]) -> String {
    hash_bytes(parts.join("\n").as_bytes())
}

impl Inputs {
    // loads the recorded stamps, or starts empty if there are none
    pub fn load(path: &Path) -> Result<Self, InputsError> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|source| InputsError::Json { path: path.into(), source }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Inputs::default()),
            Err(source) => Err(InputsError::Io { path: path.into(), source }),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), InputsError> {
        let io = |source| InputsError::Io { path: path.into(), source };
        fs::create_dir_all(path.parent().unwrap()).map_err(io)?;
        fs::write(path, serde_json::to_vec_pretty(self).unwrap()).map_err(io)
    }

    // hash of a file, reusing the recorded one while mtime and size are unchanged
    pub fn hash(&mut self, path: &Path) -> Result<String, InputsError> {
        let io = |source| InputsError::Io { path: path.into(), source };
        let metadata = fs::metadata(path).map_err(io)?;
        let mtime = metadata.modified().map_err(io)?.duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
        let size = metadata.len();

        if let Some(stamp) = self.files.get(path).filter(|stamp| stamp.mtime == mtime && stamp.size == size) {
            return Ok(stamp.hash.clone());
        }

        let hash = hash_bytes(&fs::read(path).map_err(io)?);
        self.files.insert(path.into(), Stamp { mtime, size, hash: hash.clone() });
        Ok(hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_matches_content_and_tracks_changes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.nix");
        let mut inputs = Inputs::default();

        fs::write(&file, "one").unwrap();
        assert_eq!(inputs.hash(&file).unwrap(), hash_bytes(b"one"));

        fs::write(&file, "three").unwrap();
        assert_eq!(inputs.hash(&file).unwrap(), hash_bytes(b"three"));
    }

    #[test]
    fn unchanged_stamp_reuses_recorded_hash() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.nix");
        fs::write(&file, "one").unwrap();

        // a stale hash under a matching stamp proves the file wasn't re-read
        let mut inputs = Inputs::default();
        inputs.hash(&file).unwrap();
        inputs.files.get_mut(&file).unwrap().hash = "recorded".into();
        assert_eq!(inputs.hash(&file).unwrap(), "recorded");
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.nix");
        let state = dir.path().join("state/inputs");
        fs::write(&file, "one").unwrap();

        let mut inputs = Inputs::default();
        inputs.hash(&file).unwrap();
        inputs.save(&state).unwrap();
        assert_eq!(Inputs::load(&state).unwrap().files, inputs.files);
    }

    #[test]
    fn missing_state_loads_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Inputs::load(&dir.path().join("nope")).unwrap().files.is_empty());
    }
}
