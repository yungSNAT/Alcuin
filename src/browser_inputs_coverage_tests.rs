use super::*;
use serde_json::json;
use std::fs;

fn new_session(home: &Path) -> (SessionStore, String) {
    let store = SessionStore::new(home.join(".oracle"));
    let meta = store
        .create(
            json!({"prompt":"PUBLIC-coverage upload source","mode":"browser"}),
            home,
            None,
        )
        .unwrap();
    let id = meta["id"].as_str().unwrap().to_owned();
    (store, id)
}

#[test]
fn decode_remote_base64_accepts_standard_and_url_alphabets() {
    let cases: &[(&str, &[u8])] = &[
        ("+/8=", &[0xfb, 0xff]),
        ("-_8", &[0xfb, 0xff]),
        (" + / 8 ", &[0xfb, 0xff]),
        ("+/8=AAAA", &[0xfb, 0xff]),
        ("-_-_", &[0xfb, 0xff, 0xbf]),
        ("+/+/", &[0xfb, 0xff, 0xbf]),
        ("UFVCTElDLQ==", b"PUBLIC-"),
    ];
    for (input, expected) in cases {
        assert_eq!(
            decode_remote_base64(input),
            expected.to_vec(),
            "decoding {input:?}"
        );
    }
}

#[test]
fn capture_remote_replaces_dot_and_empty_names_with_numbered_fallbacks() {
    let home = tempfile::tempdir().unwrap();
    let (store, id) = new_session(home.path());
    let attachments = vec![
        json!({"fileName": ".", "contentBase64": "UFVCTElDLQ=="}),
        json!({"fileName": "..", "contentBase64": "UFVCTElDLQ=="}),
        json!({"fileName": "", "contentBase64": "UFVCTElDLQ=="}),
    ];
    let staged = StagedUploads::capture_remote(&store, &id, &attachments).unwrap();
    assert_eq!(staged.files.len(), 3);
    let paths = staged.paths().unwrap();
    for (index, (file, path)) in staged.files.iter().zip(&paths).enumerate() {
        let expected = format!("attachment-{}", index + 1);
        assert_eq!(file.display_path, expected);
        assert!(!file.generated_bundle);
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some(expected.as_str())
        );
        assert_eq!(fs::read(path).unwrap(), b"PUBLIC-");
    }
    staged.verify(&store, &id).unwrap();
    staged.cleanup(&store, &id).unwrap();
    assert!(!staged.directory.exists());
}

#[cfg(unix)]
#[test]
fn cleanup_refuses_symlinked_staging_directory_without_removing_contents() {
    let home = tempfile::tempdir().unwrap();
    let (store, id) = new_session(home.path());
    let original = home.path().join("public.png");
    fs::write(&original, b"PUBLIC image bytes").unwrap();
    let staged = StagedUploads::capture(
        &store,
        &id,
        &[Attachment {
            path: original,
            display_path: "public.png".into(),
            size_bytes: 18,
            generated_bundle: false,
            scanned_sha256: None,
        }],
    )
    .unwrap();
    let file_name = staged.paths().unwrap()[0].file_name().unwrap().to_owned();
    let moved = store.directory(&id).unwrap().join("moved-inputs");
    fs::rename(&staged.directory, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &staged.directory).unwrap();

    let error = staged.cleanup(&store, &id).unwrap_err();
    assert!(
        !error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    );
    assert!(
        fs::symlink_metadata(&staged.directory)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read(moved.join(file_name)).unwrap(),
        b"PUBLIC image bytes"
    );
}
