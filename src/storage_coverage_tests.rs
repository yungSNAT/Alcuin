use super::*;

fn current_owner(
    process_started_at_ms: Option<f64>,
    created_at: Option<String>,
) -> (Owner, ProcessIdentity) {
    let identity = ProcessIdentity::current().unwrap();
    let record = Owner {
        id: "PUBLIC-owner".to_owned(),
        pid: identity.pid,
        created_at,
        process_started_at_ms,
        process_identity: None,
    };
    (record, identity)
}

#[test]
fn historical_owner_liveness_uses_start_time_and_creation_tolerances() {
    let identity = ProcessIdentity::current().unwrap();
    let started = identity.started_at_ms;
    let cases: Vec<(&str, Option<f64>, Option<String>, Liveness)> = vec![
        ("matching start time", Some(started), None, Liveness::Alive),
        (
            "start time far in the future",
            Some(started + 1.0e9),
            None,
            Liveness::Dead,
        ),
        (
            "created after the process started",
            None,
            Some(jiff::Timestamp::now().to_string()),
            Liveness::Alive,
        ),
        (
            "created long before the process started",
            None,
            Some("2000-01-01T00:00:00Z".to_owned()),
            Liveness::Dead,
        ),
        ("no timing evidence", None, None, Liveness::Unknown),
        (
            "unparseable creation time",
            None,
            Some("PUBLIC-not-a-timestamp".to_owned()),
            Liveness::Unknown,
        ),
    ];
    for (name, process_started_at_ms, created_at, expected) in cases {
        let (record, _) = current_owner(process_started_at_ms, created_at);
        assert!(
            owner_liveness(&record) == expected,
            "unexpected liveness for case: {name}"
        );
    }
}

#[test]
fn historical_owner_of_a_missing_process_is_dead() {
    let record = Owner {
        id: "PUBLIC-missing".to_owned(),
        // Beyond any Linux pid_max, so no such process can exist.
        pid: i32::MAX as u32,
        created_at: None,
        process_started_at_ms: Some(1.0),
        process_identity: None,
    };
    assert!(owner_liveness(&record) == Liveness::Dead);
}

#[test]
fn lock_holder_reports_pid_only_for_verifiable_small_owner_records() {
    let home = tempfile::tempdir().unwrap();

    let missing = home.path().join("PUBLIC-missing.lock");
    assert_eq!(lock_holder(&missing), "unverified ownership");

    let valid = home.path().join("PUBLIC-valid.lock");
    fs::create_dir(&valid).unwrap();
    fs::write(
        valid.join("owner.json"),
        br#"{"id":"PUBLIC-owner","pid":4242}"#,
    )
    .unwrap();
    assert_eq!(lock_holder(&valid), "pid 4242");

    let oversized = home.path().join("PUBLIC-oversized.lock");
    fs::create_dir(&oversized).unwrap();
    let mut bytes = br#"{"id":"PUBLIC-owner","pid":4242}"#.to_vec();
    bytes.extend(std::iter::repeat_n(b' ', 70 * 1024));
    fs::write(oversized.join("owner.json"), &bytes).unwrap();
    assert_eq!(lock_holder(&oversized), "unverified ownership");
    assert_eq!(fs::read(oversized.join("owner.json")).unwrap(), bytes);
}

#[test]
fn same_directory_compares_native_identities_and_rejects_missing_paths() {
    let home = tempfile::tempdir().unwrap();
    let first = home.path().join("PUBLIC-first");
    let second = home.path().join("PUBLIC-second");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();

    assert!(same_directory(&first, &first));
    assert!(same_directory(
        &first,
        &first.join("..").join("PUBLIC-first")
    ));
    assert!(!same_directory(&first, &second));
    assert!(!same_directory(&first, &home.path().join("PUBLIC-absent")));
    assert!(!same_directory(&home.path().join("PUBLIC-absent"), &first));
}

#[cfg(unix)]
#[test]
fn atomic_bytes_onto_a_directory_fails_and_removes_its_staging_file() {
    let home = tempfile::tempdir().unwrap();
    let target = home.path().join("PUBLIC-target");
    fs::create_dir(&target).unwrap();

    let error = atomic_bytes(&target, b"PUBLIC payload").unwrap_err();
    let io = error
        .downcast_ref::<std::io::Error>()
        .expect("rename failure should surface as an io::Error");
    assert!(!matches!(
        io.kind(),
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ResourceBusy
    ));
    assert!(target.is_dir());
    let names: Vec<_> = fs::read_dir(home.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 1, "staging file was left behind: {names:?}");
    assert_eq!(names[0], "PUBLIC-target");
}

#[cfg(target_os = "linux")]
#[test]
fn lock_creation_failure_other_than_contention_is_returned_immediately() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join(format!("PUBLIC-{}", "x".repeat(300)));
    let Err(error) = DirectoryLock::acquire(&path, Duration::from_secs(3)) else {
        panic!("an over-long lock name must not be acquired");
    };
    let io = error
        .downcast_ref::<std::io::Error>()
        .expect("mkdir failure should surface as an io::Error");
    assert_eq!(io.raw_os_error(), Some(libc::ENAMETOOLONG));
}

#[test]
fn dead_owner_with_unrecognised_contents_is_preserved_until_timeout() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("PUBLIC-dead.lock");
    fs::create_dir(&path).unwrap();
    atomic_json(
        &path.join("owner.json"),
        &serde_json::json!({
            "id": "PUBLIC-reused-owner",
            "pid": u32::MAX,
            "processIdentity": {"pid": u32::MAX, "nativeStart": 1, "startedAtMs": 1.0}
        }),
    )
    .unwrap();
    fs::write(path.join("PUBLIC-extra"), b"PUBLIC kept").unwrap();

    let Err(error) = DirectoryLock::acquire(&path, Duration::from_millis(10)) else {
        panic!("a lock with unrecognised contents must not be reaped");
    };
    assert!(
        error
            .to_string()
            .contains("Timed out waiting for Alcuin directory lock"),
        "unexpected error: {error:#}"
    );
    assert!(path.is_dir());
    assert_eq!(fs::read(path.join("PUBLIC-extra")).unwrap(), b"PUBLIC kept");
}

#[test]
fn publication_with_unrecognised_contents_fails_and_reports_pending_rollback() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("PUBLIC-crowded.lock");
    let mutex = RecoveryMutex::acquire(&path, Duration::from_secs(1)).unwrap();
    fs::create_dir(&path).unwrap();
    fs::write(path.join("PUBLIC-extra"), b"PUBLIC kept").unwrap();
    let directory = NativeEntry::open(&path, true).unwrap();
    let identity = ProcessIdentity::current().unwrap();
    let record = Owner {
        id: Uuid::new_v4().to_string(),
        pid: identity.pid,
        created_at: None,
        process_started_at_ms: Some(identity.started_at_ms),
        process_identity: Some(identity),
    };

    let Err(error) = DirectoryLock::publish_created(&path, directory, record, None) else {
        panic!("publication must reject unrecognised lock contents");
    };
    drop(mutex);
    assert!(
        format!("{error:#}").contains("Failed lock publication rollback remains pending"),
        "unexpected error: {error:#}"
    );
    assert!(path.is_dir());
    assert_eq!(fs::read(path.join("PUBLIC-extra")).unwrap(), b"PUBLIC kept");
}
