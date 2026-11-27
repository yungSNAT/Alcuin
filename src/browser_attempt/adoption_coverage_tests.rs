use super::*;
use std::fs;

const PAYLOAD: &[u8] = b"PUBLIC-adoption artifact\n";

fn saved_file_fixture(path: &Path) -> Value {
    fs::write(path, PAYLOAD).unwrap();
    let entry = NativeEntry::open(path, false).unwrap();
    json!({
        "kind": "file",
        "path": path,
        "sizeBytes": PAYLOAD.len(),
        "sha256": sha256(PAYLOAD),
        "publicationReceipt": {
            "path": path,
            "sizeBytes": PAYLOAD.len(),
            "sha256": sha256(PAYLOAD),
            "nativeIdentity": entry.identity,
        },
        "cleanupWarning": "PUBLIC-stale child warning",
    })
}

#[test]
fn artifact_copy_uses_receipt_identity_and_resume_reuses_the_published_file() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let source_root = root.join("child");
    fs::create_dir(&source_root).unwrap();
    let source = source_root.join("PUBLIC-report.txt");
    let artifact = saved_file_fixture(&source);
    let destination_root = root.join("parent");
    let destination = destination_root.join("PUBLIC-report.txt");
    let cancel = CancellationToken::new();

    assert!(!destination_root.exists());
    let published = copy_artifact(
        &source_root,
        &artifact,
        &destination,
        true,
        None,
        &cancel,
        None,
    )
    .unwrap();
    assert_eq!(published["path"], json!(destination));
    assert_eq!(fs::read(&destination).unwrap(), PAYLOAD);
    assert_eq!(fs::read(&source).unwrap(), PAYLOAD);
    assert_eq!(
        published["nativeIdentity"],
        json!(NativeEntry::open(&destination, false).unwrap().identity)
    );
    assert_ne!(
        published["nativeIdentity"],
        artifact["publicationReceipt"]["nativeIdentity"]
    );

    let resumed = copy_artifact(
        &source_root,
        &artifact,
        &destination,
        true,
        Some(&[]),
        &cancel,
        None,
    )
    .unwrap();
    assert_eq!(resumed["nativeIdentity"], published["nativeIdentity"]);
    assert_eq!(fs::read(&destination).unwrap(), PAYLOAD);
    assert_eq!(fs::read_dir(&destination_root).unwrap().count(), 1);

    for copy in [published, resumed] {
        assert_eq!(copy["path"], json!(destination));
        assert_eq!(copy["kind"], artifact["kind"]);
        assert_eq!(copy["publicationReceipt"]["path"], copy["path"]);
        assert_eq!(
            copy["publicationReceipt"]["nativeIdentity"],
            copy["nativeIdentity"]
        );
        assert_eq!(
            copy["publicationReceipt"]["sizeBytes"],
            artifact["sizeBytes"]
        );
        assert_eq!(copy["publicationReceipt"]["sha256"], artifact["sha256"]);
        assert!(copy.get("cleanupWarning").is_none());
    }
}

#[test]
fn artifact_copy_rejects_bad_identity_blocked_directory_and_bad_digest() {
    let temp = tempfile::tempdir().unwrap();
    let source_root = temp.path().join("child");
    fs::create_dir(&source_root).unwrap();
    let source = source_root.join("PUBLIC-report.txt");
    let artifact = saved_file_fixture(&source);
    let destination_root = temp.path().join("parent");
    let destination = destination_root.join("PUBLIC-report.txt");
    let cancel = CancellationToken::new();

    let mut malformed = artifact.clone();
    malformed["nativeIdentity"] = json!(false);
    let error = copy_artifact(
        &source_root,
        &malformed,
        &destination,
        false,
        None,
        &cancel,
        None,
    )
    .unwrap_err();
    assert!(error.to_string().contains("invalid type"), "{error:#}");
    assert!(!destination_root.exists());

    fs::write(&destination_root, b"PUBLIC-blocking file").unwrap();
    assert!(
        copy_artifact(
            &source_root,
            &artifact,
            &destination,
            false,
            None,
            &cancel,
            None,
        )
        .is_err()
    );
    assert_eq!(
        fs::read(&destination_root).unwrap(),
        b"PUBLIC-blocking file"
    );
    fs::remove_file(&destination_root).unwrap();

    let mut bad_digest = artifact;
    let wrong_hash = json!(sha256(b"PUBLIC-different content"));
    bad_digest["sha256"] = wrong_hash.clone();
    bad_digest["publicationReceipt"]["sha256"] = wrong_hash;
    let already_used: &[Value] = &[];
    for resumed in [None, Some(already_used)] {
        assert!(
            copy_artifact(
                &source_root,
                &bad_digest,
                &destination,
                false,
                resumed,
                &cancel,
                None,
            )
            .is_err()
        );
        assert!(!destination.exists());
        assert_eq!(fs::read(&source).unwrap(), PAYLOAD);
    }
}

#[test]
fn parent_capture_resume_skips_invalid_candidates_and_reuses_verified_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let store = SessionStore::new(temp.path().join("sessions"));
    let id = Uuid::new_v4().to_string();
    let directory = store.directory(&id).unwrap();
    fs::create_dir_all(&directory).unwrap();
    let cancel = CancellationToken::new();
    let answer = b"PUBLIC-right\n";
    let wrong = b"PUBLIC-wrong\n";
    assert_eq!(answer.len(), wrong.len());

    let decoys: [(String, &[u8]); 3] = [
        ("captured-answer-PUBLIC-not-a-uuid.md".into(), answer),
        (
            format!("captured-answer-{}.md", Uuid::new_v4()),
            b"PUBLIC-\n",
        ),
        (format!("captured-answer-{}.md", Uuid::new_v4()), wrong),
    ];
    for (name, bytes) in &decoys {
        fs::write(directory.join(name), bytes).unwrap();
    }
    let non_file = directory.join(format!("captured-answer-{}.md", Uuid::new_v4()));
    fs::create_dir(&non_file).unwrap();

    let saved = parent_capture(&store, &id, answer, true, &cancel, None).unwrap();
    assert_eq!(saved.session_id, id);
    assert_eq!(saved.bytes, answer.len() as u64);
    assert_eq!(saved.sha256, sha256(answer));
    assert!(decoys.iter().all(|(name, _)| name != &saved.file));
    assert_eq!(fs::read(directory.join(&saved.file)).unwrap(), answer);

    let reused = parent_capture(&store, &id, answer, true, &cancel, None).unwrap();
    assert_eq!(reused.file, saved.file);
    assert_eq!(
        artifacts::verify_capture_until(&store, &reused, Some(&cancel), None).unwrap(),
        answer
    );
    assert_eq!(fs::read_dir(&directory).unwrap().count(), decoys.len() + 2);
    for (name, bytes) in decoys {
        assert_eq!(fs::read(directory.join(name)).unwrap(), bytes);
    }
    assert!(non_file.is_dir());
}

#[test]
fn gemini_adoption_refuses_tampered_saved_observation() {
    let home = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    let mut prepared = crate::browser_runner::alternate_support_tests::prepare_parent(
        home.path(),
        "PUBLIC Gemini adoption prompt",
        json!({"browserProvider":"gemini"}),
        &cancel,
    )
    .unwrap();
    let link = prepared
        .coordinator
        .reserve_first_alternate(Provider::Gemini, &prepared.environment, &cancel)
        .unwrap();
    browser_runner::start_owner(&prepared.store, &link.child_id, &prepared.identity).unwrap();
    let store = &prepared.store;
    let parent_id = prepared.parent_id.as_str();
    let identity = &prepared.identity;
    let req = browser_runner::request(store, &link.child_id).unwrap();
    let answer = "PUBLIC tampered Gemini answer";
    let (dispatch, _observed, mut runtime) =
        crate::browser_runner::alternate_support_tests::gemini_evidence(
            &req,
            identity,
            &Uuid::new_v4().to_string(),
            "acknowledged",
            answer,
            false,
        );
    let capture = artifacts::capture(
        store,
        &link.child_id,
        "captured-gemini.md",
        answer.as_bytes(),
        &cancel,
    )
    .unwrap();
    runtime["capturedAnswer"] = json!(capture);
    store
        .update(&link.child_id, |current| {
            Ok(Some(json!({
                "dispatch": dispatch,
                "capturedAnswer": capture,
                "browser": merge_object(
                    &current["browser"],
                    &json!({"provider":"gemini","capturedAnswer":capture,"runtime":runtime}),
                )
            })))
        })
        .unwrap();

    let error = prepared
        .coordinator
        .adopt(&link.child_id, &cancel)
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("Gemini answer lacks evidence"),
        "{error:#}"
    );
    assert!(store.require(parent_id).unwrap()["capturedAnswer"].is_null());
}
