#[cfg(target_os = "linux")]
#[test]
fn scanner_accepts_a_desktop_at_the_limit_and_skips_unreadable_entries() {
    use super::{collect_open, read_desktop};
    use crate::native_fs::NativeEntry;
    use std::ffi::OsStr;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    const MAX_BYTES: usize = 1024 * 1024;

    let temp = tempfile::tempdir().unwrap();
    let browser = temp.path().join("chrome-wrapper");
    fs::write(&browser, b"PUBLIC executable fixture").unwrap();
    fs::set_permissions(&browser, fs::Permissions::from_mode(0o700)).unwrap();

    let applications = temp.path().join("PUBLIC-applications");
    fs::create_dir(&applications).unwrap();

    let at_limit = OsStr::new("PUBLIC-at-limit.desktop");
    let oversized = OsStr::new("PUBLIC-oversized.desktop");
    let invalid_utf8 = OsStr::new("PUBLIC-invalid-utf8.desktop");
    let exec_line = format!("Exec={} %U\n", browser.display());

    let mut contents = exec_line.as_bytes().to_vec();
    contents.extend_from_slice(b"# PUBLIC padding");
    contents.resize(MAX_BYTES, b' ');
    fs::write(applications.join(at_limit), &contents).unwrap();

    let directory = NativeEntry::open(&applications, true).unwrap();
    let text = read_desktop(&directory, at_limit).expect("exactly 1 MiB should be readable");
    assert_eq!(text.len(), MAX_BYTES);
    assert!(
        text.as_bytes() == contents.as_slice(),
        "the entry at the limit should be returned unchanged"
    );

    contents.push(b' ');
    fs::write(applications.join(oversized), &contents).unwrap();

    let mut invalid_contents = exec_line.into_bytes();
    invalid_contents.extend_from_slice(b"# PUBLIC invalid UTF-8: \xff\n");
    fs::write(applications.join(invalid_utf8), invalid_contents).unwrap();

    for name in [oversized, invalid_utf8] {
        assert!(read_desktop(&directory, name).is_none(), "{name:?}");
    }

    let mut paths = Vec::new();
    let mut remaining = 4;
    collect_open(
        &directory,
        &mut paths,
        &mut remaining,
        Instant::now() + Duration::from_secs(2),
        0,
    );

    assert_eq!(
        paths,
        [browser],
        "only the readable entry should contribute"
    );
    assert_eq!(remaining, 1, "all three entries should be visited");
}

#[cfg(target_os = "linux")]
#[test]
fn scanner_preserves_candidates_and_budget_when_traversal_is_blocked() {
    use super::collect_open;
    use crate::native_fs::NativeEntry;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    let temp = tempfile::tempdir().unwrap();
    let desktop = temp.path().join("PUBLIC-browser.desktop");
    fs::write(&desktop, b"# PUBLIC desktop fixture\n").unwrap();

    let directory = NativeEntry::open(temp.path(), true).unwrap();
    let regular_file = NativeEntry::from_handle(fs::File::open(&desktop).unwrap(), false).unwrap();
    let existing = PathBuf::from("PUBLIC-existing-candidate");

    for (reason, entry, depth, budget, expired, expected_remaining) in [
        ("depth beyond the limit", &directory, 33, 1, false, 1),
        ("regular file handle", &regular_file, 0, 1, false, 1),
        ("exhausted entry budget", &directory, 0, 0, false, 0),
        ("expired deadline", &directory, 0, 1, true, 1),
        ("depth at the limit", &directory, 32, 1, false, 0),
    ] {
        let now = Instant::now();
        let deadline = if expired {
            now
        } else {
            now + Duration::from_secs(2)
        };
        let mut paths = vec![existing.clone()];
        let mut remaining = budget;

        collect_open(entry, &mut paths, &mut remaining, deadline, depth);

        assert_eq!(
            paths.as_slice(),
            std::slice::from_ref(&existing),
            "{reason}"
        );
        assert_eq!(remaining, expected_remaining, "{reason}: remaining budget");
    }
}
