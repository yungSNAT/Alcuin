#![cfg(unix)]

use super::{load, resolve_existing_prefix};
use serde_json::json;
use std::{fs, io::ErrorKind, os::unix::fs::symlink, path::PathBuf};

#[test]
fn load_warns_about_unreadable_configs_and_still_loads_the_nested_project() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("PUBLIC-user-config");
    let project = root.path().join("PUBLIC-project");
    let cwd = project.join("PUBLIC-nested");
    fs::create_dir(&home).unwrap();
    fs::create_dir_all(cwd.join(".oracle")).unwrap();

    let user_config = home.join("config.json");
    fs::write(&user_config, b"PUBLIC-\xff").unwrap();

    // A regular file where a directory is expected makes metadata fail even as root.
    fs::write(project.join(".oracle"), "PUBLIC-not-a-directory").unwrap();
    let project_config = project.join(".oracle/config.json");
    let nested_config = cwd.join(".oracle/config.json");
    fs::write(&nested_config, r#"{"model":"PUBLIC-nested-model"}"#).unwrap();

    let loaded = load(&home, &cwd, Some(root.path()), true);

    assert_eq!(loaded.config, json!({"model": "PUBLIC-nested-model"}));
    assert_eq!(loaded.paths, [nested_config]);
    assert_eq!(
        loaded.warnings,
        [
            format!(
                "Failed to read {}: configuration is unreadable",
                user_config.display()
            ),
            format!(
                "Failed to inspect {}: configuration is unreadable",
                project_config.display()
            ),
        ]
    );
}

#[test]
fn resolve_existing_prefix_resolves_relative_paths_with_missing_suffixes() {
    let root = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(root.path()).unwrap();
    fs::create_dir(base.join("PUBLIC-existing")).unwrap();

    // Reach the fixture from the current directory without changing the process cwd.
    let cwd = std::env::current_dir().unwrap();
    let mut relative = PathBuf::from(".");
    for _ in cwd.components().skip(1) {
        relative.push("..");
    }
    relative.push(base.strip_prefix("/").unwrap());

    let input = relative.join("PUBLIC-existing/../PUBLIC-missing/./PUBLIC-output.json");
    assert_eq!(
        resolve_existing_prefix(&input).unwrap(),
        base.join("PUBLIC-missing/PUBLIC-output.json")
    );
}

#[test]
fn resolve_existing_prefix_names_the_dangling_symlink_in_the_error() {
    let root = tempfile::tempdir().unwrap();
    let link = fs::canonicalize(root.path()).unwrap().join("PUBLIC-link");
    symlink("PUBLIC-absent-target", &link).unwrap();

    let expected = format!(
        "MCP output path contains an unresolved symlink: {}",
        link.display()
    );
    for input in [link.clone(), link.join("PUBLIC-output.json")] {
        assert_eq!(
            resolve_existing_prefix(&input).unwrap_err().to_string(),
            expected,
            "input: {}",
            input.display()
        );
    }
}

#[test]
fn resolve_existing_prefix_preserves_the_not_a_directory_error() {
    let root = tempfile::tempdir().unwrap();
    let file = fs::canonicalize(root.path()).unwrap().join("PUBLIC-file");
    fs::write(&file, "PUBLIC-content").unwrap();

    let error = resolve_existing_prefix(&file.join("PUBLIC-output.json")).unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        ErrorKind::NotADirectory
    );
}
