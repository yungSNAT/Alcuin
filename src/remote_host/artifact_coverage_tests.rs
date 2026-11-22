use super::*;
use crate::remote_result::{self, Phase};
use serde_json::json;
use std::fs;

fn fixture(contents: &[u8]) -> (tempfile::TempDir, SessionStore, RemoteRecord, PathBuf) {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path().join(".oracle"));
    let run = uuid::Uuid::new_v4().to_string();
    let id = remote_result::session_id(&run).unwrap();
    store
        .create(
            json!({"prompt": "PUBLIC artifact verification"}),
            home.path(),
            Some(&id),
        )
        .unwrap();

    let directory = store.directory(&id).unwrap().join("artifacts");
    fs::create_dir(&directory).unwrap();
    let path = directory.join("PUBLIC-artifact.bin");
    fs::write(&path, contents).unwrap();
    let hash = Sha256::digest(contents)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let captured = json!({
        "answerText": "PUBLIC verified artifact",
        "artifacts": [{
            "kind": "file",
            "path": path,
            "sizeBytes": contents.len(),
            "sha256": hash
        }]
    });
    let produced = crate::remote_protocol::produced(&store, &run, captured, false).unwrap();
    remote_result::save(
        &store,
        &run,
        &produced.captured,
        &produced.result,
        &produced.artifacts,
        Phase::Result,
        &CancellationToken::new(),
    )
    .unwrap();
    let record = remote_result::read_phase(&store, &run, Phase::Result).unwrap();
    (home, store, record, path)
}

#[test]
fn open_propagates_digest_error_for_same_size_corruption() {
    let original = b"PUBLIC original";
    let corrupted = b"PUBLIC tampered";
    assert_eq!(original.len(), corrupted.len());
    let (_home, store, record, path) = fixture(original);
    let cancel = CancellationToken::new();
    let artifact = &record.native()["artifacts"][0];
    assert!(VerifiedArtifact::open(&store, &record, artifact, &cancel).is_ok());

    fs::write(&path, corrupted).unwrap();
    let descriptor = &artifact["descriptor"];
    let Err(expected) = artifacts::verify_streamed_file_until(
        &mut fs::File::open(&path).unwrap(),
        descriptor["byteSize"].as_u64().unwrap(),
        descriptor["sha256"].as_str().unwrap(),
        Some(&cancel),
        None,
    ) else {
        panic!("the digest verifier accepted changed bytes");
    };
    let Err(actual) = VerifiedArtifact::open(&store, &record, artifact, &cancel) else {
        panic!("opening the artifact accepted changed bytes");
    };
    assert_eq!(format!("{actual:#}"), format!("{expected:#}"));
}

#[cfg(unix)]
#[test]
fn open_respects_native_access_to_a_searchable_parent() {
    use std::os::unix::fs::PermissionsExt;

    let (_home, store, record, path) = fixture(b"PUBLIC searchable artifact");
    let canonical = path.canonicalize().unwrap();
    let parent = canonical.parent().unwrap();
    let permissions = fs::metadata(parent).unwrap().permissions();
    fs::set_permissions(parent, fs::Permissions::from_mode(0o300)).unwrap();

    let native = NativeEntry::open(parent, true);
    let result = VerifiedArtifact::open(
        &store,
        &record,
        &record.native()["artifacts"][0],
        &CancellationToken::new(),
    );
    fs::set_permissions(parent, permissions).unwrap();

    if let Err(expected) = native {
        let Err(actual) = result else {
            panic!("opening the artifact bypassed the failed native parent open");
        };
        assert_eq!(format!("{actual:#}"), format!("{expected:#}"));
    } else {
        // A privileged runner may still be able to open the directory.
        assert!(result.is_ok(), "native parent access succeeded");
    }
}

#[test]
fn stream_rejects_a_replaced_parent_before_sending_bytes() {
    let (_home, store, record, path) = fixture(b"PUBLIC retained file");
    let cancel = CancellationToken::new();
    let mut source =
        VerifiedArtifact::open(&store, &record, &record.native()["artifacts"][0], &cancel).unwrap();

    let parent = path.parent().unwrap();
    let retained = parent.with_file_name("PUBLIC-retained-directory");
    fs::rename(parent, &retained).unwrap();
    fs::create_dir(parent).unwrap();
    fs::rename(retained.join(path.file_name().unwrap()), &path).unwrap();

    // Moving the file can change its ctime. Refresh only the stamp so the
    // independent parent identity check is responsible for rejecting it.
    source.stamp = read_stamp(&source.entry.handle).unwrap();
    source.entry.require_matches(&source.canonical).unwrap();
    let expected = source
        .parent
        .require_matches(source.canonical.parent().unwrap())
        .unwrap_err();

    let (sender, mut receiver) = mpsc::channel(1);
    let actual = source.stream(sender, &cancel).unwrap_err();
    assert_eq!(format!("{actual:#}"), format!("{expected:#}"));
    assert_eq!(
        receiver.try_recv(),
        Err(mpsc::error::TryRecvError::Disconnected)
    );
}
