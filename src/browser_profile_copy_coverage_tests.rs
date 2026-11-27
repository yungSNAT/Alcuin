use super::*;
use std::time::Duration;

// On Linux, absolute paths have no CurDir component, and walking their
// normalized prefixes starts at the root. Neither branch is reachable here.

#[test]
fn cleanup_is_a_noop_after_the_retired_profile_path_is_reused() {
    let home = tempfile::tempdir().unwrap();
    let mut profile = TemporaryProfile::create(home.path()).unwrap();
    let path = profile.path().to_owned();
    fs::write(path.join("PUBLIC-owned"), b"PUBLIC-owned contents").unwrap();

    profile
        .cleanup_until(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert!(!path.try_exists().unwrap());
    assert!(profile.pins().is_none());

    fs::create_dir(&path).unwrap();
    let replacement = path.join("PUBLIC-replacement");
    fs::write(&replacement, b"PUBLIC-replacement contents").unwrap();

    profile.cleanup_until(Instant::now()).unwrap();
    assert_eq!(
        fs::read(&replacement).unwrap(),
        b"PUBLIC-replacement contents"
    );
}

#[cfg(unix)]
#[test]
fn drop_cleans_up_owned_contents_but_preserves_both_directories_on_identity_mismatch() {
    let home = tempfile::tempdir().unwrap();
    for replace_directory in [false, true] {
        let profile = TemporaryProfile::create(home.path()).unwrap();
        let path = profile.path().to_owned();
        fs::write(path.join("PUBLIC-owned"), b"PUBLIC-owned contents").unwrap();

        let moved = home.path().join("PUBLIC-moved");
        let replacement = path.join("PUBLIC-replacement");
        if replace_directory {
            fs::rename(&path, &moved).unwrap();
            fs::create_dir(&path).unwrap();
            fs::write(&replacement, b"PUBLIC-replacement contents").unwrap();
        }

        drop(profile);

        if replace_directory {
            assert_eq!(
                fs::read(&replacement).unwrap(),
                b"PUBLIC-replacement contents"
            );
            assert_eq!(
                fs::read(moved.join("PUBLIC-owned")).unwrap(),
                b"PUBLIC-owned contents"
            );
        } else {
            assert!(!path.try_exists().unwrap());
        }
    }
}
