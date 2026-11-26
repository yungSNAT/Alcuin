//! The Claude Code plugin installs the same server entry that
//! `alcuin bridge claude-config --local-browser` prints, and the repository's skill.
use alcuin::config::{self, Environment};
use serde_json::Value;
use std::{path::Path, time::Duration};

fn manifest(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".claude-plugin")
        .join(name);
    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap()
}

/// The plugin installs from this version's release tag, so its skills and server entry match
/// the released binaries. The marketplace entry carries the version, as the tagged
/// `plugin.json` has none.
#[test]
fn marketplace_installs_the_plugin_from_this_versions_tag() {
    let marketplace = manifest("marketplace.json");
    let plugin = manifest("plugin.json");
    assert_eq!(marketplace["name"], "alcuin");
    let entries = marketplace["plugins"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["name"], plugin["name"]);
    let version = env!("CARGO_PKG_VERSION");
    assert_eq!(entries[0]["version"], version);
    assert_eq!(
        entries[0]["source"],
        serde_json::json!({"source":"github","repo":"yungSNAT/Alcuin","ref":format!("v{version}")})
    );
    assert!(plugin.get("version").is_none(), "{plugin}");
    let skill = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("skills/alcuin/SKILL.md"),
    )
    .unwrap();
    let mut lines = skill.lines();
    assert_eq!(lines.next(), Some("---"));
    assert_eq!(lines.next(), Some("name: alcuin"));
}

/// The plugin's review skill previews and asks before sending, and checks what the answer
/// cites before reporting it.
#[test]
fn plugin_ships_the_review_skill() {
    let skill = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("skills/alcuin-review/SKILL.md"),
    )
    .unwrap();
    let mut lines = skill.lines();
    assert_eq!(lines.next(), Some("---"));
    assert_eq!(lines.next(), Some("name: alcuin-review"));
    for step in [
        "alcuin --engine browser --dry-run summary --files-report",
        "Show the user the provider and the file list, and wait for approval.",
        "**Leave out secrets.**",
        "don't send it again",
        "alcuin ground <id>",
        "Exit 2 means the check did not run",
        "Open every cited file and line yourself",
    ] {
        assert!(skill.contains(step), "{step}");
    }
}

#[tokio::test]
async fn plugin_server_matches_the_local_browser_claude_config() {
    let home = tempfile::tempdir().unwrap();
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_alcuin"));
    command
        .env_clear()
        .envs(std::env::var_os("LLVM_PROFILE_FILE").map(|value| ("LLVM_PROFILE_FILE", value)))
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("ORACLE_DISABLE_NOTIFICATIONS", "1")
        .current_dir(home.path())
        .kill_on_drop(true)
        .args(["bridge", "claude-config", "--local-browser"]);
    if let Ok(value) = std::env::var("SystemRoot") {
        command.env("SystemRoot", value);
    }
    let output = tokio::time::timeout(Duration::from_secs(15), command.output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let printed: Value = serde_json::from_slice(&output.stdout).unwrap();
    let mut expected = printed["mcpServers"]["alcuin"].clone();
    // The printed paths are the defaults alcuin-mcp uses when they are unset,
    // so the plugin leaves them out and works for every user's home.
    let env = expected["env"].as_object_mut().unwrap();
    let state = env.remove("ORACLE_HOME_DIR").unwrap();
    let profile = env.remove("ORACLE_BROWSER_PROFILE_DIR").unwrap();
    assert_eq!(state, serde_json::json!(home.path().join(".oracle")));
    assert_eq!(
        profile,
        serde_json::json!(home.path().join(".oracle").join("browser-profile"))
    );
    assert_eq!(manifest("plugin.json")["mcpServers"]["alcuin"], expected);
}

#[test]
fn server_defaults_match_the_paths_the_plugin_leaves_out() {
    let home = tempfile::tempdir().unwrap();
    let text = home.path().to_string_lossy().into_owned();
    let env = Environment::from_iter([
        ("HOME".into(), text.clone()),
        ("USERPROFILE".into(), text.clone()),
    ]);
    assert_eq!(
        config::oracle_home(&env).unwrap(),
        home.path().join(".oracle")
    );
    assert_eq!(
        config::manual_login_profile_dir([None], &env, &text).unwrap(),
        home.path().join(".oracle").join("browser-profile")
    );
}
