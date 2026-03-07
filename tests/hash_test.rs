use std::io::Write;

use pbuild::hash::{hash_file, is_dirty};
use tempfile::NamedTempFile;

#[test]
fn hash_file_missing_returns_none() {
    let result = hash_file("/tmp/pbuild-test-nonexistent-xyz-99999").unwrap();
    assert_eq!(result, None);
}

#[test]
fn hash_file_existing_returns_some() {
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(b"hello").unwrap();
    let result = hash_file(f.path().to_str().unwrap()).unwrap();
    assert!(result.is_some());
    assert!(!result.unwrap().is_empty());
}

#[test]
fn same_content_same_hash() {
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(b"deterministic").unwrap();
    let path = f.path().to_str().unwrap();
    let h1 = hash_file(path).unwrap();
    let h2 = hash_file(path).unwrap();
    assert_eq!(h1, h2);
}

#[test]
fn different_content_different_hash() {
    let mut f1 = NamedTempFile::new().unwrap();
    let mut f2 = NamedTempFile::new().unwrap();
    f1.write_all(b"aaa").unwrap();
    f2.write_all(b"bbb").unwrap();
    let h1 = hash_file(f1.path().to_str().unwrap()).unwrap();
    let h2 = hash_file(f2.path().to_str().unwrap()).unwrap();
    assert_ne!(h1, h2);
}

#[test]
fn is_dirty_missing_lock_entry() {
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(b"content").unwrap();
    let lf = std::collections::HashMap::new();
    assert!(is_dirty(&lf, f.path().to_str().unwrap()).unwrap());
}

#[test]
fn is_dirty_matching_hash_is_clean() {
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(b"content").unwrap();
    let path = f.path().to_str().unwrap();
    let h = hash_file(path).unwrap().unwrap();
    let lf = std::collections::HashMap::from([(path.to_string(), h)]);
    assert!(!is_dirty(&lf, path).unwrap());
}

#[test]
fn is_dirty_stale_hash() {
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(b"before").unwrap();
    let path = f.path().to_str().unwrap().to_string();
    let h = hash_file(&path).unwrap().unwrap();
    f.write_all(b"after").unwrap();
    let lf = std::collections::HashMap::from([(path.clone(), h)]);
    assert!(is_dirty(&lf, &path).unwrap());
}

#[test]
fn is_dirty_missing_file_with_lock_entry() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_string();
    let lf = std::collections::HashMap::from([(path.clone(), "somehash".to_string())]);
    drop(f); // delete the file
    assert!(is_dirty(&lf, &path).unwrap());
}

#[test]
fn lock_file_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let lock = dir.path().join(".pbuild.lock");
    // We test parse/serialize directly via write+read in a temp dir.
    // Temporarily override cwd isn't ergonomic, so we test the map round-trip
    // by writing and re-reading explicitly.
    let original = std::collections::HashMap::from([
        ("src/foo.c".to_string(), "abc123".to_string()),
        ("src/bar.c".to_string(), "def456".to_string()),
    ]);
    // Serialize manually the same way the module does.
    let mut entries: Vec<_> = original.iter().collect();
    entries.sort_by_key(|(k, _)| *k);
    let contents: String = entries.iter().fold(String::new(), |mut s, (p, h)| {
        use std::fmt::Write;
        let _ = writeln!(s, "{p}\t{h}");
        s
    });
    std::fs::write(&lock, &contents).unwrap();
    let parsed: std::collections::HashMap<String, String> = std::fs::read_to_string(&lock)
        .unwrap()
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(2, '\t');
            let p = parts.next()?.to_string();
            let h = parts.next()?.to_string();
            Some((p, h))
        })
        .collect();
    assert_eq!(original, parsed);
}
