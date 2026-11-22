#![cfg(target_os = "linux")]

use super::*;
use serde_json::json;

fn fixture() -> (tempfile::TempDir, PathBuf, Environment) {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let env = Environment::from_iter([("HOME".into(), home.to_string_lossy().into_owned())]);
    (temp, home, env)
}

fn assert_selection_error(config: &Value, env: &Environment, cwd: &Path, expected: &str) {
    match select(config, env, cwd) {
        Ok(source) => panic!("expected {expected:?}, but selected {:?}", source.database),
        Err(error) => assert_eq!(error.to_string(), expected),
    }
}

#[test]
fn normalise_removes_dot_components_without_climbing_above_root() {
    for (input, expected) in [
        ("./PUBLIC-first/../PUBLIC-profile", "PUBLIC-profile"),
        ("../../PUBLIC-profile", "PUBLIC-profile"),
        (
            "/../PUBLIC-root/PUBLIC-first/../PUBLIC-profile",
            "/PUBLIC-root/PUBLIC-profile",
        ),
    ] {
        assert_eq!(
            normalise(Path::new(input)),
            PathBuf::from(expected),
            "{input:?}"
        );
    }
}

#[test]
fn home_relative_profile_paths_require_a_database_and_detect_profile_removal() {
    let (_temp, home, env) = fixture();
    let cwd = home.join("PUBLIC-cwd");
    let user_data = home.join("PUBLIC-user-data");
    let profile = user_data.join("PUBLIC-profile");
    let network = profile.join("Network");
    fs::create_dir(&cwd).unwrap();
    fs::create_dir_all(&network).unwrap();

    let configs = [
        json!({"chromeCookiePath": "~/PUBLIC-user-data/PUBLIC-unused/../PUBLIC-profile"}),
        json!({"chromeCookiePath": "~\\PUBLIC-user-data/PUBLIC-profile"}),
    ];
    for config in &configs {
        assert_selection_error(
            config,
            &env,
            &cwd,
            "Selected Chrome profile has no cookie database",
        );
    }

    let database = network.join("Cookies");
    fs::write(&database, b"PUBLIC-synthetic-database").unwrap();
    let sources = configs.map(|config| {
        let source = select(&config, &env, &cwd).unwrap();
        assert_eq!(source.database, database);
        assert_eq!(source.user_data, user_data);
        source.verify().unwrap();
        source
    });

    fs::rename(&profile, user_data.join("PUBLIC-moved-profile")).unwrap();
    for source in sources {
        assert!(source.verify().is_err());
    }
}

#[test]
fn regular_file_at_profile_path_blocks_selection_and_makes_children_inaccessible() {
    let (_temp, home, env) = fixture();
    let root = home.join(".config/google-chrome");
    fs::create_dir_all(&root).unwrap();
    let profile = root.join("PUBLIC-profile");
    fs::write(&profile, b"PUBLIC-not-a-directory").unwrap();

    let fallback = home.join(".var/app/com.google.Chrome/config/google-chrome/PUBLIC-profile");
    fs::create_dir_all(&fallback).unwrap();
    fs::write(fallback.join("Cookies"), b"PUBLIC-fallback-database").unwrap();

    assert_selection_error(
        &json!({"chromeProfile": "PUBLIC-profile"}),
        &env,
        &home,
        "Selected Chrome profile is not a directory",
    );
    assert_selection_error(
        &json!({"chromeCookiePath": profile.join("Cookies")}),
        &env,
        &home,
        "Chrome cookie source is inaccessible",
    );
}

#[test]
fn alias_lookup_skips_roots_without_a_match_and_retains_the_matching_state() {
    let (_temp, home, env) = fixture();
    let primary = home.join(".config/google-chrome");
    fs::create_dir_all(&primary).unwrap();
    let secondary = home.join(".var/app/com.google.Chrome/config/google-chrome");
    let profile = secondary.join("PUBLIC-profile");
    fs::create_dir_all(&profile).unwrap();
    let database = profile.join("Cookies");
    fs::write(&database, b"PUBLIC-synthetic-database").unwrap();

    let state = secondary.join("Local State");
    let config = json!({"chromeProfile": "PUBLIC-display-name"});
    for primary_state in [None, Some(r#"{"profile":{"info_cache":{}}}"#)] {
        if let Some(contents) = primary_state {
            fs::write(primary.join("Local State"), contents).unwrap();
        }
        fs::write(
            &state,
            r#"{"profile":{"info_cache":{"PUBLIC-profile":{"name":"PUBLIC-display-name"}}}}"#,
        )
        .unwrap();

        let source = select(&config, &env, &home).unwrap();
        assert_eq!(source.database, database);
        assert_eq!(source.user_data, secondary);
        source.verify().unwrap();

        fs::write(&state, b"{}").unwrap();
        assert_eq!(
            source.verify().unwrap_err().to_string(),
            "Chrome profile selection changed after resolution"
        );
    }
}

#[test]
fn dot_components_are_rejected_as_direct_names_and_alias_targets() {
    let (_temp, home, env) = fixture();
    let root = home.join(".config/google-chrome");
    fs::create_dir_all(&root).unwrap();
    let state = root.join("Local State");
    let expected = "Chrome profile selection must identify one direct profile directory";

    for name in [".", ".."] {
        assert_selection_error(&json!({"chromeProfile": name}), &env, &home, expected);

        let contents = json!({
            "profile": {"info_cache": {(name): {"name": "PUBLIC-display-name"}}}
        });
        fs::write(&state, contents.to_string()).unwrap();
        assert_selection_error(
            &json!({"chromeProfile": "PUBLIC-display-name"}),
            &env,
            &home,
            expected,
        );
    }
}
