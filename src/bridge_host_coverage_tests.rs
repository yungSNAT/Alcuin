use super::*;

/// Waits for a fixture subprocess without ever blocking past a short deadline.
fn wait_for_exit(child: &mut std::process::Child) -> std::process::ExitStatus {
    let deadline = std::time::Instant::now() + Duration::from_secs(4);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("fixture subprocess did not exit in time");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Starts this test binary as one of the ignored subprocess fixtures, forwarding coverage output.
fn fixture_command(name: &str, marker: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", name, "--ignored", "--nocapture"])
        .env_clear()
        .envs(std::env::var_os("LLVM_PROFILE_FILE").map(|value| ("LLVM_PROFILE_FILE", value)))
        .env("ALCUIN_PUBLIC_TUNNEL_MARKER", marker)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

#[test]
fn split_args_keeps_trailing_and_unterminated_quoted_arguments() {
    let cases: [(&str, &[&str]); 5] = [
        ("", &[]),
        ("PUBLIC-one", &["PUBLIC-one"]),
        ("-p 2222", &["-p", "2222"]),
        (
            "-o 'PUBLIC unterminated value",
            &["-o", "PUBLIC unterminated value"],
        ),
        (
            "  -o \"User=PUBLIC name\" -v",
            &["-o", "User=PUBLIC name", "-v"],
        ),
    ];
    for (raw, expected) in cases {
        assert_eq!(split_args(raw), expected, "split_args({raw:?})");
    }
}

#[test]
fn ssh_args_build_reverse_forward_with_optional_identity_and_extra_args() {
    let base = [
        "-N",
        "-R",
        "2201:127.0.0.1:9473",
        "-o",
        "ExitOnForwardFailure=yes",
        "-o",
        "ServerAliveInterval=30",
        "-o",
        "ServerAliveCountMax=3",
    ];
    let minimal = ssh_args(
        &json!({"ssh":" PUBLIC-user@PUBLIC-host ","ssh-identity":"   ","ssh-extra-args":""}),
        2201,
        9473,
    );
    let mut expected: Vec<String> = base.iter().map(|value| value.to_string()).collect();
    expected.push("PUBLIC-user@PUBLIC-host".into());
    assert_eq!(minimal, expected);

    let full = ssh_args(
        &json!({
            "ssh":"PUBLIC-user@PUBLIC-host",
            "ssh-identity":" /tmp/PUBLIC-key ",
            "ssh-extra-args":"-p 2222 -o 'User=PUBLIC name'"
        }),
        2201,
        9473,
    );
    let mut expected: Vec<String> = base.iter().map(|value| value.to_string()).collect();
    expected.extend(
        [
            "-i",
            "/tmp/PUBLIC-key",
            "-p",
            "2222",
            "-o",
            "User=PUBLIC name",
            "PUBLIC-user@PUBLIC-host",
        ]
        .map(String::from),
    );
    assert_eq!(full, expected);
}

#[cfg(unix)]
#[tokio::test]
async fn reverse_tunnel_stops_before_spawning_when_cancelled_and_reports_spawn_failure() {
    let home = tempfile::tempdir().unwrap();
    let missing = home.path().join("PUBLIC-missing-ssh");
    let env = Environment::from_iter([("PUBLIC_NAME".into(), "PUBLIC-value".into())]);

    let cancelled = CancellationToken::new();
    cancelled.cancel();
    // A missing executable would fail to spawn, so success proves the early return.
    reverse_tunnel(
        missing.clone(),
        vec!["-N".into()],
        env.clone(),
        home.path().into(),
        cancelled,
    )
    .await
    .unwrap();

    let error = reverse_tunnel(
        missing,
        vec!["-N".into()],
        env,
        home.path().into(),
        CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("Failed to start reverse SSH tunnel"),
        "unexpected error: {error:#}"
    );
}

#[cfg(unix)]
#[test]
fn kill_tree_falls_back_to_killing_a_child_without_its_own_process_group() {
    use std::os::unix::process::ExitStatusExt;
    // Without setsid the child's pid is not a process group, so group kill answers ESRCH.
    let child = std::process::Command::new("sleep")
        .arg("5")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut owner = StartingHost {
        child: Some(child),
        armed: true,
    };
    owner.kill_tree().unwrap();
    assert!(!owner.armed);
    // A disarmed owner is a no-op.
    owner.kill_tree().unwrap();
    let mut child = owner.child.take().unwrap();
    let status = wait_for_exit(&mut child);
    assert_eq!(status.signal(), Some(libc::SIGKILL));
}

#[test]
fn tunnel_child_fixture_records_agent_marker_and_attempt_before_exiting() {
    let home = tempfile::tempdir().unwrap();
    let marker = home.path().join("PUBLIC-fixture.pid");
    let mut child = fixture_command("bridge_host::tests::tunnel_child_fixture", &marker)
        .env("ALCUIN_PUBLIC_TUNNEL_EXIT", "1")
        .env("SSH_AUTH_SOCK", "PUBLIC-agent-socket")
        .spawn()
        .unwrap();
    let pid = child.id();
    let status = wait_for_exit(&mut child);
    assert_eq!(status.code(), Some(7));
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), pid.to_string());
    assert_eq!(
        std::fs::read_to_string(marker.with_extension("agent")).unwrap(),
        "PUBLIC-agent-socket"
    );
    assert_eq!(
        std::fs::read_to_string(marker.with_extension("attempts")).unwrap(),
        format!("{pid}\n")
    );
}

#[test]
fn startup_tree_fixture_rejects_a_gate_other_than_ready() {
    use std::io::Write;
    let home = tempfile::tempdir().unwrap();
    let marker = home.path().join("PUBLIC-startup.pid");
    let mut child = fixture_command("bridge_host::tests::startup_tree_fixture", &marker)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let mut gate = child.stdin.take().unwrap();
    gate.write_all(b"PUBLIC-not-ready").unwrap();
    drop(gate);
    let status = wait_for_exit(&mut child);
    assert!(!status.success());
    assert_eq!(status.code(), Some(101));
    assert!(!marker.exists(), "no descendant may start after a bad gate");
}

#[cfg(unix)]
#[test]
fn tunnel_proxy_fixture_publishes_its_root_and_exits_naturally() {
    use std::os::unix::process::CommandExt;
    let home = tempfile::tempdir().unwrap();
    let marker = home.path().join("PUBLIC-proxy.pid");
    let mut child = fixture_command("bridge_host::tests::tunnel_proxy_fixture", &marker)
        .env("ALCUIN_PUBLIC_TUNNEL_NATURAL", "1")
        .process_group(0)
        .spawn()
        .unwrap();
    let pid = child.id();
    let status = wait_for_exit(&mut child);
    assert_eq!(status.code(), Some(7));
    assert_eq!(
        std::fs::read_to_string(marker.with_extension("root")).unwrap(),
        pid.to_string()
    );
    // The proxy's long-lived descendant shares its process group; stop it.
    // SAFETY: kill takes no pointers; the group is the one this test started.
    let killed = unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
    assert_eq!(killed, 0, "proxy descendant should still be running");
}
