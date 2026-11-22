use super::*;
use std::{ffi::OsStr, io::Read};

#[test]
fn open_child_rejects_dot_components_and_reads_a_normal_child() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("parent");
    fs::create_dir(&path).unwrap();
    fs::write(path.join("record"), b"PUBLIC-child").unwrap();
    let directory = NativeEntry::open(&path, true).unwrap();

    for name in [".", ".."] {
        let error = directory
            .open_child(&path, OsStr::new(name), true)
            .err()
            .expect("a dot component must be rejected");
        assert_eq!(
            error.to_string(),
            "Expected one native child component",
            "component: {name:?}"
        );
    }

    let mut child = directory
        .open_child(&path, OsStr::new("record"), false)
        .unwrap();
    let mut contents = Vec::new();
    child.handle.read_to_end(&mut contents).unwrap();
    assert_eq!(contents, b"PUBLIC-child");
}

#[test]
fn directory_retirement_preserves_entries_when_an_expected_file_is_missing() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("owned");
    fs::create_dir(&path).unwrap();

    let kept = path.join("kept");
    let missing = path.join("missing");
    fs::write(&kept, b"PUBLIC-kept").unwrap();
    fs::write(&missing, b"PUBLIC-moved").unwrap();

    let directory = NativeEntry::open(&path, true).unwrap();
    let kept_entry = NativeEntry::open(&kept, false).unwrap();
    let missing_entry = NativeEntry::open(&missing, false).unwrap();
    let files = [("kept", &kept_entry), ("missing", &missing_entry)];
    recognised_contents(&path, &directory, &files).unwrap();

    let moved = root.path().join("moved");
    fs::rename(&missing, &moved).unwrap();

    let error = remove_owned_directory(&path, &directory, &files).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("Directory contents changed; preserving {}", path.display())
    );
    assert!(directory.matches(&path));
    assert!(kept_entry.matches(&kept));
    assert!(missing_entry.matches(&moved));
    assert_eq!(fs::read(&kept).unwrap(), b"PUBLIC-kept");
    assert_eq!(fs::read(&moved).unwrap(), b"PUBLIC-moved");
    assert_eq!(fs::read_dir(&path).unwrap().count(), 1);
}
