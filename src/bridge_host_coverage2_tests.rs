#![cfg(target_os = "linux")]

use super::*;
use crate::process::ProcessIdentity;
use tokio::{io::AsyncWriteExt, process::Command, time::Instant};

const OPTIONS_FILE: &str = "PUBLIC-options.json";
const ERROR_FILE: &str = "PUBLIC-error.txt";
const CONNECTION_FILE: &str = "PUBLIC-connection.json";
const PUBLIC_TOKEN: &str = "PUBLIC-bridge-token";

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn identity_line(identity: &ProcessIdentity) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(identity).unwrap();
    bytes.push(b'\n');
    bytes
}

// Give the host a private stdin without changing the test runner's stdin or environment.
#[tokio::test]
#[ignore = "requires piped stdin and a temporary working directory"]
async fn startup_gate_fixture_records_host_error() {
    let home = std::env::current_dir().unwrap();
    let options = read_json(&home.join(OPTIONS_FILE));
    let env = Environment::from_iter([
        ("HOME".into(), home.to_str().unwrap().into()),
        ("ORACLE_BRIDGE_START_GATE".into(), "1".into()),
    ]);
    // Keep the host's first printed line separate from the test harness prefix.
    println!();
    let error = run(
        &options,
        SessionStore::new(home.clone()),
        home.clone(),
        env,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    std::fs::write(home.join(ERROR_FILE), format!("{error:#}")).unwrap();
}

async fn run_gate_fixture(
    home: &Path,
    options: &Value,
    input: impl FnOnce(u32) -> Vec<u8>,
    deadline: Instant,
) -> (String, String) {
    std::fs::write(
        home.join(OPTIONS_FILE),
        serde_json::to_vec(options).unwrap(),
    )
    .unwrap();
    let output = tokio::time::timeout_at(deadline, async {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "bridge_host::coverage2_tests::startup_gate_fixture_records_host_error",
                "--ignored",
                "--nocapture",
            ])
            .current_dir(home)
            .env_clear()
            .envs(std::env::var_os("LLVM_PROFILE_FILE").map(|value| ("LLVM_PROFILE_FILE", value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let bytes = input(child.id().unwrap());
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(&bytes).await.unwrap();
        drop(stdin);
        child.wait_with_output().await.unwrap()
    })
    .await
    .expect("startup gate fixture exceeded its deadline");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        output.status.success(),
        "startup gate fixture failed: {}\nstdout:\n{stdout}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let error = std::fs::read_to_string(home.join(ERROR_FILE)).unwrap();
    (error, stdout)
}

#[tokio::test]
async fn start_gate_rejects_external_startup_oversized_input_and_foreign_identity() {
    let deadline = Instant::now() + Duration::from_secs(4);
    let foreign_identity = identity_line(&ProcessIdentity::current().unwrap());
    for (respawn, input, expected) in [
        (
            false,
            Vec::new(),
            "Background bridge start gate requires internal respawn",
        ),
        (
            true,
            vec![b' '; 8193],
            "Background bridge start gate identity mismatch",
        ),
        (
            true,
            foreign_identity,
            "Background bridge start gate identity mismatch",
        ),
    ] {
        let home = tempfile::tempdir().unwrap();
        let options = json!({
            "respawn": respawn,
            "token": PUBLIC_TOKEN,
            "write-connection": CONNECTION_FILE
        });
        let (error, _) = run_gate_fixture(home.path(), &options, |_| input, deadline).await;
        assert_eq!(error, expected);
        assert!(!home.path().join(CONNECTION_FILE).exists());
        assert!(!home.path().join("bridge-host-owner.json").exists());
    }
}

#[tokio::test]
async fn connection_artifact_and_requested_output_survive_background_log_failure() {
    let deadline = Instant::now() + Duration::from_secs(4);
    for with_tunnel in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("bridge-host.log");
        // Opening a directory as an append-only log fails before any host or tunnel is spawned.
        std::fs::create_dir(&log).unwrap();
        let bind = if with_tunnel {
            "127.0.0.1:9473"
        } else {
            "0.0.0.0:9473"
        };
        let mut options = json!({
            "bind": bind,
            "background": true,
            "respawn": true,
            "token": PUBLIC_TOKEN,
            "write-connection": CONNECTION_FILE,
            "print": with_tunnel,
            "print-token": with_tunnel
        });
        if with_tunnel {
            options["ssh"] = json!("  PUBLIC-user@PUBLIC-host  ");
            options["ssh-remote-port"] = json!("2222");
            options["ssh-identity"] = json!("  PUBLIC-key  ");
            options["ssh-extra-args"] = json!("  -o 'User=PUBLIC-user'  ");
        }

        let (error, stdout) = run_gate_fixture(
            home.path(),
            &options,
            |pid| identity_line(&ProcessIdentity::read(pid).unwrap().unwrap()),
            deadline,
        )
        .await;
        let log_error = std::io::Error::from_raw_os_error(libc::EISDIR).to_string();
        assert!(error.contains(&log_error), "unexpected error: {error}");
        assert!(log.is_dir());
        assert!(!home.path().join("bridge-host.pid").exists());
        assert!(!home.path().join("bridge-host-owner.json").exists());

        let connection = read_json(&home.path().join(CONNECTION_FILE));
        assert_eq!(connection["remoteToken"], PUBLIC_TOKEN);
        if with_tunnel {
            assert_eq!(connection["remoteHost"], "127.0.0.1:2222");
            assert_eq!(
                connection["tunnel"],
                json!({
                    "ssh": "PUBLIC-user@PUBLIC-host",
                    "remotePort": 2222,
                    "localPort": 9473,
                    "identity": "PUBLIC-key",
                    "extraArgs": "-o 'User=PUBLIC-user'"
                })
            );
            assert!(stdout.lines().any(|line| line == PUBLIC_TOKEN), "{stdout}");
            let formatted = bridge::format_connection(&connection, true).unwrap();
            assert!(stdout.contains(&formatted), "{stdout}");
        } else {
            assert_eq!(connection["remoteHost"], "127.0.0.1:9473");
            assert!(connection.get("tunnel").is_none());
            assert!(!stdout.contains(PUBLIC_TOKEN), "{stdout}");
        }
    }
}

#[tokio::test]
async fn gated_startup_does_not_publish_readiness_without_an_owner_and_commit() {
    let deadline = Instant::now() + Duration::from_secs(4);
    let bind = "127.0.0.1:0";
    let parsed = bridge::parse_host_port(bind);
    if let Ok((hostname, port)) = &parsed {
        assert_eq!(hostname, "127.0.0.1");
        assert_eq!(*port, 0);
    }

    for block_owner in [true, false] {
        let home = tempfile::tempdir().unwrap();
        let owner = home.path().join("bridge-host-owner.json");
        if block_owner {
            std::fs::create_dir(&owner).unwrap();
        }
        let connection = home.path().join(CONNECTION_FILE);
        let options = json!({
            "bind": bind,
            "respawn": true,
            "token": PUBLIC_TOKEN,
            "write-connection": CONNECTION_FILE
        });
        let (error, _) = run_gate_fixture(
            home.path(),
            &options,
            |pid| {
                // Send the identity, but close stdin without committing the handoff.
                identity_line(&ProcessIdentity::read(pid).unwrap().unwrap())
            },
            deadline,
        )
        .await;
        assert!(!home.path().join("bridge-host-ready.json").exists());
        // If ephemeral binds are rejected, verify that the rejection precedes artifact
        // publication rather than substituting a fixed port.
        if let Err(bind_error) = &parsed {
            assert_eq!(error, format!("{bind_error:#}"));
            assert!(!connection.exists());
            return;
        }

        assert_eq!(read_json(&connection)["remoteToken"], PUBLIC_TOKEN);
        if block_owner {
            assert!(
                error.starts_with("Bridge owner publication failed; shutdown: Ok(())"),
                "{error}"
            );
            assert!(owner.is_dir());
        } else {
            assert_eq!(
                error,
                "Background handoff failed; shutdown: Ok(()): Background bridge handoff was not committed"
            );
            let published_owner = read_json(&owner);
            assert_eq!(published_owner["connectionArtifact"], json!(connection));
            assert!(published_owner["processIdentity"].is_object());
        }
    }
}
