#![cfg(unix)]

use super::*;
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::PermissionsExt,
    time::Duration,
};

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(5)
}

fn running_as_root() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

fn failure<T>(result: Result<T>, context: &str) -> String {
    match result {
        Ok(_) => panic!("{context}"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn component_accepts_only_one_normal_name() {
    let cases: [(&str, bool); 7] = [
        ("PUBLIC-profile", true),
        ("PUBLIC-cookie.db", true),
        ("", false),
        (".", false),
        ("..", false),
        ("/", false),
        ("PUBLIC-a/PUBLIC-b", false),
    ];
    for (name, accepted) in cases {
        let result = component(OsStr::new(name));
        assert_eq!(result.is_ok(), accepted, "unexpected verdict for {name:?}");
        if let Err(error) = result {
            assert_eq!(
                error.to_string(),
                "Expected one private profile component",
                "unexpected message for {name:?}"
            );
        }
    }
}

#[test]
fn create_directory_and_file_report_existing_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let root = Directory::open_parent(tmp.path()).unwrap();

    let profile = root.create_directory(OsStr::new("PUBLIC-profile")).unwrap();
    assert_eq!(profile.path, tmp.path().join("PUBLIC-profile"));
    assert!(profile.path.is_dir());
    let mode = fs::metadata(&profile.path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode & 0o077, 0, "profile directory must be private");

    let message = failure(
        root.create_directory(OsStr::new("PUBLIC-profile")),
        "second mkdir must fail",
    );
    assert!(
        message.starts_with("Cannot create private profile directory: "),
        "{message}"
    );
    assert!(message.contains("exists"), "{message}");

    let mut file = profile.create_file(OsStr::new("PUBLIC-cookie")).unwrap();
    file.write_all(b"PUBLIC-data").unwrap();
    drop(file);
    let mut contents = String::new();
    fs::File::open(profile.path.join("PUBLIC-cookie"))
        .unwrap()
        .read_to_string(&mut contents)
        .unwrap();
    assert_eq!(contents, "PUBLIC-data");

    let message = failure(
        profile.create_file(OsStr::new("PUBLIC-cookie")),
        "exclusive create must fail",
    );
    assert!(
        message.starts_with("Cannot create private profile file: "),
        "{message}"
    );
    assert!(message.contains("exists"), "{message}");

    let message = failure(
        profile.create_file(OsStr::new("..")),
        "non-normal component must be rejected",
    );
    assert_eq!(message, "Expected one private profile component");
}

#[test]
fn remove_empty_refuses_non_empty_profile_until_contents_retired() {
    let tmp = tempfile::tempdir().unwrap();
    let root = Directory::open_parent(tmp.path()).unwrap();
    let profile = root.create_directory(OsStr::new("PUBLIC-profile")).unwrap();
    profile
        .create_file(OsStr::new("PUBLIC-cookie"))
        .unwrap()
        .write_all(b"PUBLIC-data")
        .unwrap();
    let nested = profile
        .create_directory(OsStr::new("PUBLIC-nested"))
        .unwrap();
    nested.create_file(OsStr::new("PUBLIC-inner")).unwrap();

    let message = failure(
        profile.remove_empty(&root, deadline()),
        "non-empty directory must not be removed",
    );
    assert!(
        message.starts_with("Cannot remove empty private profile: "),
        "{message}"
    );
    assert!(profile.path.is_dir());
    assert!(profile.path.join("PUBLIC-cookie").is_file());

    profile.remove_contents(deadline(), 0).unwrap();
    assert!(fs::read_dir(&profile.path).unwrap().next().is_none());

    profile.remove_empty(&root, deadline()).unwrap();
    assert!(!profile.path.exists());
}

#[test]
fn remove_contents_reports_unlink_and_enumeration_failures() {
    let tmp = tempfile::tempdir().unwrap();
    let root = Directory::open_parent(tmp.path()).unwrap();
    let locked = root.create_directory(OsStr::new("PUBLIC-locked")).unwrap();
    locked.create_file(OsStr::new("PUBLIC-leaf")).unwrap();
    let leaf = locked.path.join("PUBLIC-leaf");

    // Read and search only: enumeration works, unlinking the child does not.
    fs::set_permissions(&locked.path, fs::Permissions::from_mode(0o500)).unwrap();
    let unlink_result = locked.remove_contents(deadline(), 0);
    fs::set_permissions(&locked.path, fs::Permissions::from_mode(0o700)).unwrap();

    if running_as_root() {
        assert!(unlink_result.is_ok());
        assert!(!leaf.exists());
        locked.create_file(OsStr::new("PUBLIC-leaf")).unwrap();
    } else {
        let message = failure(unlink_result, "unlink must fail");
        assert!(
            message.starts_with("Cannot retire private profile child: "),
            "{message}"
        );
        assert!(leaf.is_file());
    }

    // No permissions at all: a fresh enumeration description cannot be opened.
    fs::set_permissions(&locked.path, fs::Permissions::from_mode(0o000)).unwrap();
    let enumerate_result = locked.remove_contents(deadline(), 0);
    fs::set_permissions(&locked.path, fs::Permissions::from_mode(0o700)).unwrap();

    if running_as_root() {
        assert!(enumerate_result.is_ok());
        assert!(!leaf.exists());
    } else {
        let message = failure(enumerate_result, "enumeration must fail");
        assert!(
            message.starts_with("Cannot enumerate private profile: "),
            "{message}"
        );
        assert!(leaf.is_file());
    }
}
