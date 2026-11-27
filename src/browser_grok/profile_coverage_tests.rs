use super::*;

#[test]
fn child_config_rejects_a_profile_file_and_accepts_distinct_directories() {
    let root = tempfile::tempdir().unwrap();
    let environment = Environment::from([
        (
            "ORACLE_HOME_DIR".into(),
            root.path().to_string_lossy().into_owned(),
        ),
        (
            (if cfg!(windows) { "USERPROFILE" } else { "HOME" }).into(),
            root.path().to_string_lossy().into_owned(),
        ),
    ]);
    let parent = json!({
        "manualLoginProfileDir": "PUBLIC-chatgpt",
        "copyProfileSource": "PUBLIC-copy-source",
    });
    let grok = root.path().join("grok-browser-profile");
    for name in ["PUBLIC-chatgpt", "PUBLIC-copy-source"] {
        fs::create_dir(root.path().join(name)).unwrap();
    }

    fs::write(&grok, "PUBLIC-profile-placeholder").unwrap();
    let error = child_config(&parent, &environment, root.path()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Browser profile ancestor is not a directory"
    );
    assert_eq!(
        fs::read_to_string(&grok).unwrap(),
        "PUBLIC-profile-placeholder"
    );

    fs::remove_file(&grok).unwrap();
    fs::create_dir(&grok).unwrap();
    let config = child_config(&parent, &environment, root.path()).unwrap();
    assert_eq!(
        config["manualLoginProfileDir"],
        json!(crate::native_fs::canonical_wire_path(&grok).unwrap())
    );
}

#[cfg(target_os = "linux")]
#[test]
fn candidate_propagates_not_a_directory_for_an_intermediate_file() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("PUBLIC-regular-file");
    fs::write(&file, "PUBLIC-fixture").unwrap();

    let error = candidate(&file.join("PUBLIC-profile")).unwrap_err();
    let io_error = error.downcast_ref::<std::io::Error>().unwrap();
    assert_eq!(io_error.kind(), std::io::ErrorKind::NotADirectory);
}
