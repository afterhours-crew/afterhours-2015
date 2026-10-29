use super::*;
use std::sync::{
    Arc, Barrier,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "afterhours-prepared-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn key(revision: u32) -> Key {
    Key::new(&"a".repeat(64), "constructed", revision, b"inputs").unwrap()
}
fn decode(bytes: &[u8]) -> Result<Vec<u8>, Error> {
    if bytes == b"valid model" {
        Ok(bytes.to_vec())
    } else {
        Err(Error::Cache("invalid model"))
    }
}
fn get(
    root: &Path,
    key: &Key,
    build: impl FnOnce() -> Result<Vec<u8>, Error>,
) -> Result<Prepared<Vec<u8>>, Error> {
    prepare(
        root,
        key,
        100,
        Duration::from_secs(2),
        || false,
        decode,
        build,
    )
}

#[test]
fn warm_restart_loads_without_building_and_revision_invalidates() {
    let root = Temp::new();
    assert_eq!(
        get(&root.0, &key(1), || Ok(b"valid model".to_vec()))
            .unwrap()
            .source,
        Source::Missing
    );
    let hit = get(&root.0, &key(1), || panic!("warm compilation")).unwrap();
    assert_eq!(hit.source, Source::Hit);
    assert_eq!(hit.value, b"valid model");
    assert_eq!(
        get(&root.0, &key(2), || Ok(b"valid model".to_vec()))
            .unwrap()
            .source,
        Source::Missing
    );
    assert_ne!(
        key(1).0,
        Key::new(&"b".repeat(64), "constructed", 1, b"inputs")
            .unwrap()
            .0
    );
    assert_ne!(
        key(1).0,
        Key::new(&"a".repeat(64), "constructed", 1, b"changed")
            .unwrap()
            .0
    );
}

#[test]
fn corruption_and_truncation_rebuild_but_invalid_models_never_publish() {
    let root = Temp::new();
    let key = key(1);
    get(&root.0, &key, || Ok(b"valid model".to_vec())).unwrap();
    let path = root.0.join(format!("{}.bin", key.0));
    let original = std::fs::read(&path).unwrap();
    let mut corrupted = original.clone();
    *corrupted.last_mut().unwrap() ^= 1;
    for broken in [
        corrupted,
        original[..original.len() - 1].to_vec(),
        vec![255; 4],
    ] {
        std::fs::write(&path, &broken).unwrap();
        assert!(get(&root.0, &key, || Ok(b"invalid model".to_vec())).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), broken);
        assert_eq!(
            get(&root.0, &key, || Ok(b"valid model".to_vec()))
                .unwrap()
                .source,
            Source::Invalid
        );
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
}

#[test]
fn simultaneous_builders_publish_once() {
    let root = Temp::new();
    let barrier = Arc::new(Barrier::new(2));
    let count = Arc::new(AtomicUsize::new(0));
    let handles = (0..2)
        .map(|_| {
            let (root, barrier, count) = (root.0.clone(), barrier.clone(), count.clone());
            std::thread::spawn(move || {
                barrier.wait();
                get(&root, &key(1), || {
                    count.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(30));
                    Ok(b"valid model".to_vec())
                })
                .unwrap()
                .value
            })
        })
        .collect::<Vec<_>>();
    for handle in handles {
        assert_eq!(handle.join().unwrap(), b"valid model");
    }
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[test]
fn bounds_cancellation_and_builder_failure_leave_no_published_payload() {
    let root = Temp::new();
    let key = key(1);
    assert!(get(&root.0, &key, || Err(Error::Cache("build failed"))).is_err());
    assert!(get(&root.0, &key, || Ok(vec![0; 101])).is_err());
    let cancel = AtomicBool::new(false);
    assert!(
        prepare(
            &root.0,
            &key,
            100,
            Duration::ZERO,
            || cancel.load(Ordering::SeqCst),
            decode,
            || {
                cancel.store(true, Ordering::SeqCst);
                Ok(b"valid model".to_vec())
            }
        )
        .is_err()
    );
    assert!(!root.0.join(format!("{}.bin", key.0)).exists());
    assert!(
        std::fs::read_dir(&root.0)
            .unwrap()
            .all(|p| p.unwrap().path().extension().unwrap() == "lock")
    );
}

#[test]
fn another_builder_has_a_bounded_wait_and_unexpected_paths_are_preserved() {
    let root = Temp::new();
    let key = key(1);
    let lock = File::create(root.0.join(format!("{}.lock", key.0))).unwrap();
    lock.lock().unwrap();
    assert!(
        prepare(
            &root.0,
            &key,
            100,
            Duration::ZERO,
            || false,
            decode,
            || panic!("must wait")
        )
        .is_err()
    );
    drop(lock);
    let path = root.0.join(format!("{}.bin", key.0));
    std::fs::create_dir(&path).unwrap();
    assert!(get(&root.0, &key, || panic!("unexpected directory")).is_err());
    assert!(path.is_dir());
}

#[test]
fn valid_digest_does_not_accept_an_invalid_model_or_another_key() {
    let root = Temp::new();
    let first = key(1);
    // A structurally valid older representation can have a valid digest and
    // still fail the current component's semantic validation.
    prepare(
        &root.0,
        &first,
        100,
        Duration::ZERO,
        || false,
        |bytes| Ok(bytes.to_vec()),
        || Ok(b"obsolete model".to_vec()),
    )
    .unwrap();
    let rebuilt = get(&root.0, &first, || Ok(b"valid model".to_vec())).unwrap();
    assert_eq!(rebuilt.source, Source::Invalid);
    let other = key(2);
    std::fs::copy(
        root.0.join(format!("{}.bin", first.0)),
        root.0.join(format!("{}.bin", other.0)),
    )
    .unwrap();
    assert_eq!(
        get(&root.0, &other, || Ok(b"valid model".to_vec()))
            .unwrap()
            .source,
        Source::Invalid
    );
}

#[test]
fn cancelled_before_preparation_does_not_create_a_cache_directory() {
    let root = Temp::new();
    let absent = root.0.join("cancelled");
    assert!(
        prepare(
            &absent,
            &key(1),
            100,
            Duration::ZERO,
            || true,
            decode,
            || panic!("cancelled preparation must not build"),
        )
        .is_err()
    );
    assert!(!absent.exists());
}
