use super::*;
use crate::{
    config::LoadedConfig,
    remote_host::{HostOptions, RemoteHost, RunDriver},
    sessions::SessionStore,
};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

fn environment(pairs: &[(&str, &str)]) -> Environment {
    Environment::from_iter(
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned())),
    )
}

fn loaded(config: Value, paths: Vec<PathBuf>) -> LoadedConfig {
    LoadedConfig {
        config,
        paths,
        warnings: vec!["PUBLIC-loaded-warning".into()],
    }
}

fn prepared_store(root: &Path) -> SessionStore {
    let store = SessionStore::new(root.join("oracle"));
    crate::storage::private_dir(store.home()).unwrap();
    store
}

fn has_line(text: &str, expected: &str) -> bool {
    text.lines().any(|line| line == expected)
}

async fn serve_health(body: &'static str) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buffer = [0u8; 1024];
                loop {
                    match stream.read(&mut buffer).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => {
                            request.extend_from_slice(&buffer[..read]);
                            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                                break;
                            }
                        }
                    }
                }
                let length = body.len();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {length}\r\nconnection: close\r\n\r\n{body}"
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    address
}

async fn serve_closing() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            drop(stream);
        }
    });
    address
}

#[test]
fn format_connection_rejects_invalid_hosts_and_missing_fields() {
    let cases = [
        (
            json!({"remoteHost":"PUBLIC-no-port","remoteToken":"PUBLIC-t"}),
            false,
            "Invalid host:port format: PUBLIC-no-port",
        ),
        (
            json!({"remoteHost":"PUBLIC-host:0","remoteToken":"PUBLIC-t"}),
            true,
            "Invalid port: \"0\"",
        ),
        (
            json!({"remoteToken":"PUBLIC-t"}),
            false,
            "Connection host is missing",
        ),
        (
            json!({"remoteHost":"PUBLIC-host:9473"}),
            true,
            "Connection token is missing",
        ),
    ];
    for (connection, include_token, expected) in cases {
        let error = format_connection(&connection, include_token)
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{error}");
    }
    assert_eq!(
        format_connection(&json!({"remoteHost":"[::1]:80"}), false).unwrap(),
        "oracle+tcp://[::1]:80"
    );
}

#[test]
fn resolve_prefers_trimmed_environment_when_config_is_blank() {
    let env = environment(&[("ORACLE_REMOTE_HOST", " PUBLIC-env-host:9473 ")]);
    let resolved = resolve(&json!({"browser":{"remoteHost":"   "}}), &env);
    assert_eq!(
        resolved,
        json!({"sources":{"host":"env","token":"unset"},"host":"PUBLIC-env-host:9473"})
    );
}

#[cfg(unix)]
#[test]
fn claude_snippet_includes_remote_values_and_default_paths() {
    let env = environment(&[("HOME", "PUBLIC-home")]);
    let config =
        json!({"browser":{"remoteHost":"PUBLIC-remote:9473","remoteToken":"PUBLIC-secret-token"}});
    let text = mcp_snippet("claude-config", &json!({"print-token":true}), &config, &env).unwrap();
    let parsed: Value = serde_json::from_str(&text).unwrap();
    let server = &parsed["mcpServers"]["alcuin"];
    assert_eq!(server["type"], "stdio");
    assert_eq!(server["command"], "alcuin-mcp");
    assert_eq!(server["args"], json!([]));
    let default = Path::new("PUBLIC-home").join(".oracle-local");
    assert_eq!(server["env"]["ORACLE_ENGINE"], "browser");
    assert_eq!(server["env"]["ORACLE_HOME_DIR"], json!(default));
    assert_eq!(
        server["env"]["ORACLE_BROWSER_PROFILE_DIR"],
        json!(default.join("browser-profile"))
    );
    assert_eq!(server["env"]["ORACLE_REMOTE_HOST"], "PUBLIC-remote:9473");
    assert_eq!(server["env"]["ORACLE_REMOTE_TOKEN"], "PUBLIC-secret-token");

    let without_remote: Value =
        serde_json::from_str(&mcp_snippet("claude-config", &json!({}), &json!({}), &env).unwrap())
            .unwrap();
    assert!(
        without_remote["mcpServers"]["alcuin"]["env"]
            .get("ORACLE_REMOTE_HOST")
            .is_none()
    );

    let no_home = mcp_snippet("claude-config", &json!({}), &json!({}), &environment(&[]))
        .unwrap_err()
        .to_string();
    assert_eq!(no_home, "MCP configuration has no user home");
    let unknown = mcp_snippet("PUBLIC-kind", &json!({}), &json!({}), &env)
        .unwrap_err()
        .to_string();
    assert_eq!(unknown, "Unknown MCP configuration kind");
}

#[test]
fn resolve_connection_parses_bare_strings_and_reports_bad_paths() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        resolve_connection("127.0.0.1:9473?token=PUBLIC-token", dir.path()).unwrap(),
        json!({"remoteHost":"127.0.0.1:9473","remoteToken":"PUBLIC-token"})
    );
    let missing = resolve_connection("PUBLIC-missing.json", dir.path())
        .unwrap_err()
        .to_string();
    assert!(
        missing.starts_with("Connection artifact not found at")
            && missing.contains("PUBLIC-missing.json"),
        "{missing}"
    );
    std::fs::create_dir(dir.path().join("PUBLIC-dir.json")).unwrap();
    let not_file = resolve_connection("PUBLIC-dir.json", dir.path())
        .unwrap_err()
        .to_string();
    assert!(not_file.ends_with("but it is not a file."), "{not_file}");
}

#[tokio::test]
async fn configure_client_requires_connect_value() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path().join("oracle"));
    let cancel = CancellationToken::new();
    for options in [json!({}), json!({"connect":"   "}), json!({"connect":42})] {
        let error = configure_client(&options, &store, dir.path(), &cancel)
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            "Missing --connect. Provide a connection string or a bridge-connection.json path."
        );
    }
}

#[tokio::test]
async fn configure_client_writes_tunnel_and_handles_null_missing_and_unreadable_configs() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path().join("oracle"));
    let cancel = CancellationToken::new();

    std::fs::write(dir.path().join("PUBLIC-config.json"), "null").unwrap();
    let artifact = json!({
        "remoteHost":"127.0.0.1:9473",
        "remoteToken":"PUBLIC-tunnel-token",
        "tunnel":{
            "ssh":"PUBLIC-user@PUBLIC-host",
            "remotePort":9473,
            "localPort":9474,
            "identity":"PUBLIC-identity",
            "extraArgs":["-o","PUBLIC-option"],
            "ignored":"PUBLIC-ignored"
        }
    });
    std::fs::write(
        dir.path().join("bridge-connection.json"),
        serde_json::to_vec(&artifact).unwrap(),
    )
    .unwrap();
    let options =
        json!({"connect":"bridge-connection.json","no-test":true,"config":"PUBLIC-config.json"});
    let output = configure_client(&options, &store, dir.path(), &cancel)
        .await
        .unwrap();
    assert!(
        has_line(&output, "Wrote remote config to PUBLIC-config.json"),
        "{output}"
    );
    assert!(!output.contains("Remote service OK"));
    let bytes = std::fs::read(dir.path().join("PUBLIC-config.json")).unwrap();
    assert_eq!(bytes.last(), Some(&b'\n'));
    let saved: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        saved,
        json!({"browser":{
            "remoteHost":"127.0.0.1:9473",
            "remoteToken":"PUBLIC-tunnel-token",
            "remoteViaSshReverseTunnel":{
                "ssh":"PUBLIC-user@PUBLIC-host",
                "remotePort":9473,
                "localPort":9474,
                "identity":"PUBLIC-identity",
                "extraArgs":["-o","PUBLIC-option"]
            }
        }})
    );

    let fresh = json!({"connect":"oracle+tcp://127.0.0.1:9473?token=PUBLIC-fresh-token","no-test":true,"config":"PUBLIC-fresh.json"});
    let output = configure_client(&fresh, &store, dir.path(), &cancel)
        .await
        .unwrap();
    assert!(
        has_line(&output, "Wrote remote config to PUBLIC-fresh.json"),
        "{output}"
    );
    let saved: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("PUBLIC-fresh.json")).unwrap())
            .unwrap();
    assert_eq!(
        saved,
        json!({"browser":{"remoteHost":"127.0.0.1:9473","remoteToken":"PUBLIC-fresh-token"}})
    );

    std::fs::create_dir(dir.path().join("PUBLIC-config-dir")).unwrap();
    let unreadable = json!({"connect":"oracle+tcp://127.0.0.1:9473?token=PUBLIC-dir-token","no-test":true,"config":"PUBLIC-config-dir"});
    let error = configure_client(&unreadable, &store, dir.path(), &cancel)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.starts_with("Failed to read PUBLIC-config-dir: "),
        "{error}"
    );
}

#[tokio::test]
async fn health_without_artifact_transfer_reports_manual_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let store = prepared_store(dir.path());
    let cancel = CancellationToken::new();
    let address = serve_health(r#"{"ok":true,"version":"PUBLIC-9.9.9"}"#).await;
    let host = address.to_string();

    let config = json!({"browser":{"remoteHost":host,"remoteToken":"PUBLIC-fake-token"}});
    let (text, code) = doctor(&loaded(config, vec![]), &store, &environment(&[]), &cancel)
        .await
        .unwrap();
    assert_eq!(code, 0, "{text}");
    for expected in [
        "Remote service: configured",
        "remoteToken: set (config.browser)",
        "TCP connect: ok",
        "Auth (/health): oracle PUBLIC-9.9.9",
        "Artifact transfer: manual fallback",
        "- Remote host does not advertise bridge artifact transfer; ChatGPT-generated files may need manual copy from the browser host.",
        "- PUBLIC-loaded-warning",
    ] {
        assert!(has_line(&text, expected), "missing {expected:?} in {text}");
    }
    assert!(!text.contains("PUBLIC-fake-token"));
    assert!(!text.contains("Problems:"));

    let options = json!({"connect":format!("oracle+tcp://{host}?token=PUBLIC-fake-token"),"no-write-config":true});
    let output = configure_client(&options, &store, dir.path(), &cancel)
        .await
        .unwrap();
    let expected = format!(
        "Remote service OK ({host}) — oracle PUBLIC-9.9.9 — artifact transfer unavailable; file downloads require manual copy"
    );
    assert!(has_line(&output, &expected), "{output}");
    assert!(!output.contains("Wrote remote config"));
}

#[tokio::test]
async fn doctor_reports_auth_failures_and_redacts_token() {
    let dir = tempfile::tempdir().unwrap();
    let store = prepared_store(dir.path());
    let cancel = CancellationToken::new();
    let driver: RunDriver =
        Arc::new(|_| Box::pin(async { anyhow::bail!("PUBLIC health must not dispatch") }));
    let remote = RemoteHost::start(
        SessionStore::new(dir.path().join("producer")),
        HostOptions {
            address: "127.0.0.1:0".parse().unwrap(),
            token: "PUBLIC-correct-token".into(),
            max_concurrent_runs: None,
            max_queued_runs: None,
            max_browser_tabs: 1,
        },
        driver,
    )
    .await
    .unwrap();
    let closing = serve_closing().await;
    let cases = [
        (closing.to_string(), "PUBLIC-dropped-token"),
        (remote.address.to_string(), "PUBLIC-wrong-token"),
    ];
    for (host, token) in cases {
        let config = json!({"browser":{"remoteHost":host,"remoteToken":token}});
        let (text, code) = doctor(&loaded(config, vec![]), &store, &environment(&[]), &cancel)
            .await
            .unwrap();
        assert_eq!(code, 1, "{text}");
        assert!(has_line(&text, "TCP connect: ok"), "{text}");
        assert!(
            text.lines()
                .any(|line| line.starts_with("Auth (/health): ")),
            "{text}"
        );
        assert!(
            text.lines()
                .any(|line| line.starts_with("- Remote auth failed: ")),
            "{text}"
        );
        assert!(has_line(&text, "Problems:"), "{text}");
        assert!(!text.contains(token), "{text}");
    }
    remote.shutdown().await.unwrap();
}

#[tokio::test]
async fn doctor_lists_project_configs_defaults_and_missing_remote_token() {
    let dir = tempfile::tempdir().unwrap();
    let store = prepared_store(dir.path());
    let cancel = CancellationToken::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let host = listener.local_addr().unwrap().to_string();
    let config_path = store.home().join("config.json");
    let first = dir.path().join("PUBLIC-first.json");
    let second = dir.path().join("PUBLIC-second.json");
    let config = json!({"engine":"browser","model":"PUBLIC-model","browser":{"remoteHost":host}});
    let env = environment(&[("HOME", "PUBLIC-home")]);
    let cases = [
        (
            vec![config_path.clone(), first.clone()],
            format!("Project config: {}", first.display()),
            "",
        ),
        (
            vec![first.clone(), second.clone()],
            format!("Project configs: {}, {}", first.display(), second.display()),
            " (missing)",
        ),
    ];
    for (paths, projects, missing) in cases {
        let (text, code) = doctor(&loaded(config.clone(), paths), &store, &env, &cancel)
            .await
            .unwrap();
        assert_eq!(code, 1, "{text}");
        for expected in [
            projects,
            format!("Config: {}{missing}", config_path.display()),
            "Default engine: browser".into(),
            "Default model: PUBLIC-model".into(),
            format!("remoteHost: {host} (config.browser)"),
            "remoteToken: missing (unset)".into(),
            "TCP connect: ok".into(),
            "- Remote token is missing. Run `alcuin bridge client --connect <...>` or set ORACLE_REMOTE_TOKEN.".into(),
            "- PUBLIC-loaded-warning".into(),
        ] {
            assert!(has_line(&text, &expected), "missing {expected:?} in {text}");
        }
        assert!(!text.contains("Auth (/health)"));
    }
    drop(listener);
}

#[cfg(unix)]
#[tokio::test]
async fn doctor_without_remote_reports_local_browser_detection() {
    let dir = tempfile::tempdir().unwrap();
    let store = prepared_store(dir.path());
    let cancel = CancellationToken::new();
    let home = dir.path().join("PUBLIC-home");
    for relative in [
        ".config/google-chrome/Default/Cookies",
        ".config/google-chrome/Default/Network/Cookies",
        ".config/chromium/Default/Cookies",
        ".config/chromium/Default/Network/Cookies",
    ] {
        let path = home.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"").unwrap();
    }
    let env = environment(&[("HOME", home.to_str().unwrap())]);
    let (text, code) = doctor(&loaded(json!({}), vec![]), &store, &env, &cancel)
        .await
        .unwrap();
    assert!(has_line(&text, "Remote service: not configured"), "{text}");
    assert!(
        text.lines().any(|line| line.starts_with("Chrome: ")),
        "{text}"
    );
    let chrome_missing = has_line(&text, "Chrome: not found");
    assert_eq!(
        chrome_missing,
        text.contains("- No Chrome installation detected. Install Chrome/Chromium or set --browser-chrome-path.")
    );
    assert_eq!(code, i32::from(chrome_missing));
    assert!(
        text.lines().any(|line| line.starts_with("Cookies DB: ")),
        "{text}"
    );
    let cookies_missing = has_line(&text, "Cookies DB: not found");
    assert_eq!(
        cookies_missing,
        text.contains("Chrome cookies DB not detected")
    );
    assert!(text.contains("[mcp_servers.alcuin]"));
    assert!(has_line(&text, "- PUBLIC-loaded-warning"));
}
