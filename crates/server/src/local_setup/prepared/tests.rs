use super::*;
use nfs_content::synthetic;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "afterhours-local-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture(root: &Path) -> (crate::install_content::Generated, Documents) {
    let game = root.join("game");
    let (options, _) = synthetic::write(&game, &synthetic::assets()).unwrap();
    let generated = crate::install_content::resolve(root, &game, None, &options).unwrap();
    let mut documents: Documents = policy::documents()
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
    documents.insert(
        "garage-logic".into(),
        crate::garage_logic::tests::content_json(),
    );
    documents.insert(
        "world-content".into(),
        serde_json::from_slice(&std::fs::read(&generated.world_content).unwrap()).unwrap(),
    );
    let types = [
        "ChallengeType_GetXAmountOfREP",
        "ChallengeType_DoXDoughnuts",
        "ChallengeType_GetXNearMisses",
    ]
    .into_iter()
    .enumerate()
    .map(|(i, k)| (k.to_owned(), i as u32))
    .collect();
    documents.insert(
        "challenge-content".into(),
        policy::challenges(&types).unwrap(),
    );
    (generated, documents)
}

#[test]
fn unchanged_configuration_reuses_validated_documents_without_opening_the_installation() {
    let root = Temp::new();
    let (generated, expected) = fixture(&root.0);
    let first = documents(&root.0, &generated, || Ok(expected.clone())).unwrap();
    let warm = documents(&root.0, &generated, || {
        panic!("warm configuration extraction")
    })
    .unwrap();
    assert_eq!(first, expected);
    assert_eq!(warm, expected);
    let mut changed = generated.clone();
    changed.prepared.fingerprint.digest = "b".repeat(64);
    let count = std::cell::Cell::new(0);
    documents(&root.0, &changed, || {
        count.set(count.get() + 1);
        Ok(expected.clone())
    })
    .unwrap();
    assert_eq!(count.get(), 1);
    let fixed = policy::documents()
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
    let before = key(&generated.prepared.fingerprint.digest, &fixed).unwrap();
    let mut changed_policy = fixed;
    changed_policy.insert("new-policy".into(), json!({"revision":2}));
    let after = key(&generated.prepared.fingerprint.digest, &changed_policy).unwrap();
    assert_ne!(format!("{before:?}"), format!("{after:?}"));
}

#[test]
fn cached_documents_reject_missing_unknown_changed_policy_and_invalid_runtime_models() {
    let root = Temp::new();
    let (generated, valid) = fixture(&root.0);
    let fixed = policy::documents()
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
    let scenes = nfs_content::scenes::Catalog::load(&generated.scene_catalog).unwrap();
    assert!(decode(&serde_json::to_vec(&valid).unwrap(), &fixed, &scenes).is_ok());
    let mut cases = Vec::new();
    let mut missing = valid.clone();
    missing.remove("garage-logic");
    cases.push(missing);
    let mut unknown = valid.clone();
    unknown.insert("../other".into(), json!({}));
    cases.push(unknown);
    let mut changed = valid.clone();
    changed.insert("auth-config".into(), json!({}));
    cases.push(changed);
    for name in ["garage-logic", "world-content", "challenge-content"] {
        let mut invalid = valid.clone();
        invalid.insert(name.into(), json!({}));
        cases.push(invalid);
    }
    for invalid in cases {
        assert!(decode(&serde_json::to_vec(&invalid).unwrap(), &fixed, &scenes).is_err());
    }
}

#[test]
fn concurrent_publication_reuses_one_generation_and_preserves_modified_configuration() {
    let root = Temp::new();
    let (generated, valid) = fixture(&root.0);
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    // The byte comparison is the same for initial publication and a
                    // competing publisher; neither may overwrite an existing file.
                    publish_documents(&root.0, &generated, &valid).unwrap()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results[0], results[1]);
    let path = PathBuf::from(results[0]["files"]["garage-logic"].as_str().unwrap());
    std::fs::write(&path, b"owner modification").unwrap();
    assert!(publish_documents(&root.0, &generated, &valid).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"owner modification");
    let paths = std::fs::read_dir(root.0.join("artifacts/local-config")).unwrap();
    assert_eq!(paths.count(), 1);
}
