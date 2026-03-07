use std::collections::HashMap;
use std::fs;
use std::io;

use sha2::{Digest, Sha256};

pub type FileHash = String;
pub type LockFile = HashMap<String, FileHash>;

const LOCK_PATH: &str = ".pbuild.lock";

/// Hash a file's contents with SHA-256. Returns `None` if the file does not exist.
pub fn hash_file(path: &str) -> io::Result<Option<FileHash>> {
    match fs::read(path) {
        Ok(bytes) => {
            let digest = Sha256::digest(&bytes);
            Ok(Some(hex::encode(digest)))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Read the lock file from disk. Returns an empty map if absent.
pub fn read_lock_file() -> io::Result<LockFile> {
    match fs::read_to_string(LOCK_PATH) {
        Ok(contents) => Ok(parse_lock_file(&contents)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(e) => Err(e),
    }
}

/// Write the lock file to disk.
pub fn write_lock_file(lf: &LockFile) -> io::Result<()> {
    let mut entries: Vec<_> = lf.iter().collect();
    entries.sort_by_key(|(k, _)| *k);
    let contents = entries.iter().fold(String::new(), |mut s, (p, h)| {
        use std::fmt::Write;
        let _ = writeln!(s, "{p}\t{h}");
        s
    });
    fs::write(LOCK_PATH, contents)
}

/// True if the file's current hash differs from the stored hash.
/// A missing file or missing lock entry is always dirty.
pub fn is_dirty(lf: &LockFile, path: &str) -> io::Result<bool> {
    let current = hash_file(path)?;
    Ok(current.as_deref() != lf.get(path).map(String::as_str))
}

/// Hash the current value of an environment variable.
/// Returns `None` if the variable is unset.
#[must_use]
pub fn hash_env(var: &str) -> Option<FileHash> {
    let val = std::env::var(var).ok()?;
    let digest = Sha256::digest(val.as_bytes());
    Some(hex::encode(digest))
}

/// Lock file key for an environment variable.
#[must_use]
pub fn env_key(var: &str) -> String {
    format!("env:{var}")
}

/// True if the env var's current value differs from the stored hash.
/// An unset variable with no lock entry is clean; any other mismatch is dirty.
pub fn env_is_dirty(lf: &LockFile, var: &str) -> bool {
    let key = env_key(var);
    hash_env(var).as_deref() != lf.get(&key).map(String::as_str)
}

fn parse_lock_file(s: &str) -> LockFile {
    s.lines()
        .filter_map(|line| {
            let (path, hash) = line.split_once('\t')?;
            Some((path.to_string(), hash.to_string()))
        })
        .collect()
}
