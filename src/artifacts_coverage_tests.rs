use super::*;

#[cfg(unix)]
#[test]
fn capture_rejects_changed_canonical_root_and_retains_saved_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let ancestor = temp.path().join("PUBLIC-ancestor");
    let moved = temp.path().join("PUBLIC-moved");
    let home = ancestor.join("PUBLIC-home");
    fs::create_dir_all(&home).unwrap();
    let store = SessionStore::new(&home);
    let metadata = store
        .create(
            serde_json::json!({"prompt": "PUBLIC-capture", "model": "gpt-6-astra"}),
            &home,
            None,
        )
        .unwrap();
    let id = metadata["id"].as_str().unwrap();
    let previous_root = fs::canonicalize(store.root()).unwrap();
    let file = "PUBLIC-answer.md";
    let target = store.directory(id).unwrap().join(file);
    let bytes = b"PUBLIC-saved-answer";

    let error = capture_with(&store, id, file, bytes, &CancellationToken::new(), || {
        // Preserve access to the same files through the old paths while changing
        // their canonical location. The symlink is above the store root.
        fs::rename(&ancestor, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &ancestor).unwrap();
    })
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "The captured answer bytes or storage identity changed while saving."
    );
    assert_ne!(fs::canonicalize(store.root()).unwrap(), previous_root);
    assert_eq!(fs::read(target).unwrap(), bytes);
}

#[cfg(target_os = "linux")]
#[test]
fn payload_path_too_long_removes_the_empty_stage_in_both_entry_points() {
    let temp = tempfile::tempdir().unwrap();
    // Allow the stage directory itself to open, but make its payload path
    // exceed Linux's pathname limit without exceeding any component limit.
    let stage_name_len = ".alcuin-publish-00000000-0000-0000-0000-000000000000".len();
    let parent_len = libc::PATH_MAX as usize - 2 - stage_name_len;
    let mut parent = fs::canonicalize(temp.path()).unwrap();
    let prefix_len = "/PUBLIC-".len();
    assert!(parent.as_os_str().len() + prefix_len <= parent_len);
    let component = format!("PUBLIC-{}", "x".repeat(120));
    while parent.as_os_str().len() + 200 < parent_len {
        parent.push(&component);
    }
    let padding = parent_len - parent.as_os_str().len() - prefix_len;
    parent.push(format!("PUBLIC-{}", "x".repeat(padding)));
    fs::create_dir_all(&parent).unwrap();
    let destination = parent.join("PUBLIC-answer.md");

    let assert_failed_and_cleaned = |error: anyhow::Error| {
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .and_then(std::io::Error::raw_os_error),
            Some(libc::ENAMETOOLONG),
            "{error:#}"
        );
        assert!(fs::read_dir(&parent).unwrap().next().is_none());
    };

    let Err(error) = ArtifactStage::create(&destination) else {
        panic!("opening the staged payload unexpectedly succeeded");
    };
    assert_failed_and_cleaned(error);

    let error = publish_with(
        b"PUBLIC-payload",
        &destination,
        false,
        &CancellationToken::new(),
        |_| panic!("publication hook ran without a staged payload"),
        |stage| {
            assert_eq!(stage.as_os_str().len(), libc::PATH_MAX as usize - 1);
            assert!(fs::read_dir(stage).unwrap().next().is_none());
        },
    )
    .unwrap_err();
    assert_failed_and_cleaned(error);
}

#[test]
fn abort_reports_cleanup_failure_without_losing_the_original_error() {
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("PUBLIC-answer.md");
    let stage = ArtifactStage::create(&destination).unwrap();
    let extra = stage.directory_path.join("PUBLIC-unrecognized");
    fs::write(&extra, b"PUBLIC-preserve").unwrap();

    let error = stage.abort(anyhow::anyhow!("PUBLIC-original-error"));

    assert!(
        error
            .to_string()
            .starts_with("Artifact staging cleanup also failed: ")
    );
    assert_eq!(error.root_cause().to_string(), "PUBLIC-original-error");
    assert_eq!(fs::read(extra).unwrap(), b"PUBLIC-preserve");
    assert!(!destination.exists());
}

#[test]
fn dropping_an_unpublished_stage_removes_only_its_staging_directory() {
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("PUBLIC-answer.md");
    fs::write(&destination, b"PUBLIC-existing-output").unwrap();
    let mut stage = ArtifactStage::create(&destination).unwrap();
    stage.write(b"PUBLIC-staged-output").unwrap();
    let directory = stage.directory_path.clone();
    assert_eq!(
        fs::read(directory.join("payload")).unwrap(),
        b"PUBLIC-staged-output"
    );

    drop(stage);

    assert!(!directory.exists());
    assert_eq!(fs::read(destination).unwrap(), b"PUBLIC-existing-output");
}

#[test]
fn publication_retains_output_when_readback_size_or_digest_differs() {
    let original = b"PUBLIC-original";
    for replacement in [b"PUBLIC-modified".as_slice(), b"PUBLIC-short"] {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("PUBLIC-answer.md");

        let error = publish_with(
            original,
            &destination,
            false,
            &CancellationToken::new(),
            |stage| fs::write(stage.join("payload"), replacement).unwrap(),
            |_| {},
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            format!(
                "Cannot verify published artifact at {}; output is retained.",
                destination.display()
            )
        );
        assert_eq!(fs::read(destination).unwrap(), replacement);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }
}
