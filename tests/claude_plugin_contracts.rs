//! The Claude Code plugin installs the same server entry that
//! `alcuin bridge claude-config --local-browser` prints, and the repository's skill.
use serde_json::Value;
use std::{path::Path, time::Duration};

fn manifest(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".claude-plugin")
        .join(name);
    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap()
}

#[test]
fn marketplace_lists_the_repository_root_as_the_alcuin_plugin() {
    let marketplace = manifest("marketplace.json");
    let plugin = manifest("plugin.json");
    assert_eq!(marketplace["name"], "alcuin");
    let entries = marketplace["plugins"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["name"], plugin["name"]);
    assert_eq!(entries[0]["source"], "./");
    let skill = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("skills/alcuin/SKILL.md"),
    )
    .unwrap();
    let mut lines = skill.lines();
    assert_eq!(lines.next(), Some("---"));
    assert_eq!(lines.next(), Some("name: alcuin"));
}

#[tokio::test]
async fn plugin_server_matches_the_local_browser_claude_config() {
    let home = tempfile::tempdir().unwrap();
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_alcuin"));
    command
        .env_clear()
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
