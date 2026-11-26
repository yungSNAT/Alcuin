use super::*;

const CAPTURE: &[u8] = b"PUBLIC-captured artifact";

fn capture_receipt(path: &Path) -> Value {
    fs::write(path, CAPTURE).unwrap();
    let entry = NativeEntry::open(path, false).unwrap();
    json!({
        "path": path,
        "sizeBytes": CAPTURE.len(),
        "sha256": sha256(CAPTURE),
        "nativeIdentity": entry.identity,
    })
}

#[test]
fn legacy_answer_receipts_return_raw_bytes_without_validating_heading_or_capture_hash() {
    let required = json!({"captureSha256": "PUBLIC-legacy hash placeholder"});
    let metadata = json!({"model": "gpt-6-pro"});
    for capture in [b"PUBLIC-legacy answer\r\n\xff".as_slice(), b""] {
        assert_eq!(
            required_answer_bytes(&required, &metadata, capture).unwrap(),
            capture
        );
    }
}

#[cfg(target_os = "linux")]
fn assert_errno<T>(result: Result<T>, expected: i32) {
    let Err(error) = result else {
        panic!("expected filesystem error {expected}, but the operation succeeded");
    };
    assert_eq!(
        error
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::raw_os_error),
        Some(expected),
        "{error:#}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn output_path_errors_preserve_the_blocking_file_and_leave_no_staged_output() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path().join("store"));
    store.ensure_storage().unwrap();
    let output = home.path().join("output");
    fs::create_dir(&output).unwrap();
    let blocker = home.path().join("PUBLIC-blocker");
    let source = capture_receipt(&blocker);
    let blocked = blocker.join("PUBLIC-answer.md");
    let overlong = output.join(format!("PUBLIC-{}", "x".repeat(4096)));
    let overlong_parent = overlong.join("PUBLIC-answer.md");
    let cancel = CancellationToken::new();

    // Exercise both an invalid destination filename and an invalid ancestor.
    for target in [&overlong, &overlong_parent] {
        assert_errno(
            save_inspection(&store, target, CAPTURE, &cancel),
            libc::ENAMETOOLONG,
        );
    }
    assert_errno(
        resume_saved_file(&source, &blocked, false, None, &[], &cancel, None),
        libc::ENOTDIR,
    );

    assert_eq!(fs::read(&blocker).unwrap(), CAPTURE);
    assert!(fs::read_dir(&output).unwrap().next().is_none());
}

#[cfg(target_os = "linux")]
#[test]
fn recapture_aborts_and_removes_its_stage_when_the_source_exceeds_its_receipt() {
    let home = tempfile::tempdir().unwrap();
    let destination = home.path().join("PUBLIC-export.txt");
    // This read-only procfs file reports zero bytes in stat but returns text when read.
    let source = PathBuf::from(format!("/proc/{}/status", std::process::id()));
    assert_eq!(fs::metadata(&source).unwrap().len(), 0);
    let receipt = json!({"path": source, "sizeBytes": 0, "sha256": sha256(b"")});

    let Err(error) = recapture_saved_file(&receipt, &destination, false, &CancellationToken::new())
    else {
        panic!("recapture accepted bytes beyond the receipt's size");
    };
    assert!(
        format!("{error:#}").contains("Saved artifact exceeded its receipt"),
        "{error:#}"
    );
    assert!(fs::read_dir(home.path()).unwrap().next().is_none());
}

#[test]
fn exact_destination_resume_publishes_once_and_rejects_conflicting_bytes() {
    let home = tempfile::tempdir().unwrap();
    let root = canonical_wire_path(home.path()).unwrap();
    let source = capture_receipt(&root.join("PUBLIC-source.bin"));
    let destination = root.join("PUBLIC-export.bin");
    let cancel = CancellationToken::new();
    let deliver = || resume_saved_file(&source, &destination, false, None, &[], &cancel, None);

    let published = deliver().unwrap();
    assert_eq!(published.path, destination);
    assert_eq!(published.size_bytes, CAPTURE.len() as u64);
    assert_eq!(published.sha256, sha256(CAPTURE));
    assert_eq!(fs::read(&destination).unwrap(), CAPTURE);
    assert_eq!(
        published.native_identity,
        NativeEntry::open(&destination, false).unwrap().identity
    );

    let rebound = deliver().unwrap();
    assert_eq!(rebound.path, published.path);
    assert_eq!(rebound.size_bytes, published.size_bytes);
    assert_eq!(rebound.sha256, published.sha256);
    assert_eq!(rebound.native_identity, published.native_identity);
    assert!(rebound.cleanup_error.is_none());

    let mut conflicting = CAPTURE.to_vec();
    *conflicting.last_mut().unwrap() = b'!';
    fs::write(&destination, &conflicting).unwrap();
    assert!(
        deliver().is_err(),
        "accepted different bytes of the same size"
    );
    assert_eq!(fs::read(&destination).unwrap(), conflicting);

    let shorter = b"PUBLIC-shorter";
    fs::write(&destination, shorter).unwrap();
    let Err(error) = deliver() else {
        panic!("accepted a destination with a different size");
    };
    assert_eq!(error.to_string(), "Saved file size changed");
    assert_eq!(fs::read(&destination).unwrap(), shorter);
    assert_eq!(fs::read(root.join("PUBLIC-source.bin")).unwrap(), CAPTURE);
    assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
}

#[cfg(unix)]
#[test]
fn resumed_exports_use_the_source_basename_and_reuse_only_eligible_copies() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("PUBLIC-Report.csv");
    let mut receipt = capture_receipt(&source);
    receipt["mimeType"] = json!("text/csv");
    let output = home.path().join("output");
    fs::create_dir(&output).unwrap();
    let output = canonical_wire_path(&output).unwrap();
    let answer = output.join("public-report.csv");
    fs::write(&answer, CAPTURE).unwrap();

    for name in [
        "public-report-extra.csv",
        "public-report-1.csv",
        "public-report-02.csv",
    ] {
        fs::write(output.join(name), CAPTURE).unwrap();
    }
    let link = output.join("public-report-2.csv");
    std::os::unix::fs::symlink(&source, &link).unwrap();
    let directory = output.join("public-report-3.csv");
    fs::create_dir(&directory).unwrap();

    let copies = [
        output.join("public-report-5.csv"),
        output.join("public-report-7.csv"),
    ];
    for copy in &copies {
        fs::write(copy, CAPTURE).unwrap();
    }
    let initial_count = fs::read_dir(&output).unwrap().count();

    let cancel = CancellationToken::new();
    let exports =
        resume_exports(Some(&answer), &[receipt.clone(), receipt], &cancel, None).unwrap();
    assert_eq!(exports.len(), copies.len());
    for (row, copy) in exports.iter().zip(&copies) {
        assert_eq!(Path::new(row["path"].as_str().unwrap()), copy);
        assert_eq!(row["label"], "public-report.csv (write-output)");
        assert_eq!(row["mimeType"], "text/csv");
        assert_eq!(row["sizeBytes"], CAPTURE.len());
        assert_eq!(row["sha256"], sha256(CAPTURE));
        assert_eq!(
            row["nativeIdentity"],
            json!(NativeEntry::open(copy, false).unwrap().identity)
        );
        assert_eq!(saved_file_bytes(row, &cancel).unwrap(), CAPTURE);
    }
    assert_eq!(fs::read_dir(&output).unwrap().count(), initial_count);
    assert_eq!(fs::read(&answer).unwrap(), CAPTURE);
    assert_eq!(fs::read_link(&link).unwrap(), source);
    assert!(directory.is_dir());
}
