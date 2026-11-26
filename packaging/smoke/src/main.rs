//! Runs a packaged `alcuin` and `alcuin-mcp` in a scratch home: the version, an offline dry run
//! and the MCP handshake with its tool list.
//!
//! Usage: `alcuin-smoke <folder holding both> <version>`

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, ExitCode, Output, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const TOOLS: [&str; 5] = ["consult", "ground", "project_sources", "sessions", "wait"];
const MCP_TIMEOUT: Duration = Duration::from_secs(60);
const MCP_INPUT: &str = concat!(
    r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}"#,
    "\n",
    r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    "\n",
    r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
    "\n",
);

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let [dir, version] = args.as_slice() else {
        eprintln!("Usage: alcuin-smoke <folder holding alcuin and alcuin-mcp> <version>");
        return ExitCode::from(2);
    };
    match smoke(Path::new(dir), version) {
        Ok(()) => {
            println!("smoke test passed: {dir}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("smoke test failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn smoke(dir: &Path, version: &str) -> Result<(), String> {
    let dir = fs::canonicalize(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let exe = if dir.join("alcuin.exe").is_file() {
        ".exe"
    } else {
        ""
    };
    let cli = dir.join(format!("alcuin{exe}"));
    let mcp = dir.join(format!("alcuin-mcp{exe}"));
    let home = scratch_home()?;
    let command = |program: &Path| {
        let mut command = Command::new(program);
        command
            .current_dir(&home)
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("ORACLE_HOME_DIR", home.join(".oracle"))
            .env("ORACLE_DISABLE_NOTIFICATIONS", "1");
        command
    };

    let reported = stdout(&run(command(&cli).arg("--version"))?).replace('\r', "");
    let reported = reported.trim_end_matches('\n');
    if reported != version {
        return Err(format!(
            "alcuin --version printed {reported:?}, not {version:?}"
        ));
    }

    fs::write(home.join("smoke.rs"), "pub fn smoke() {}\n").map_err(|e| e.to_string())?;
    let dry_run = stdout(&run(command(&cli).args([
        "--dry-run",
        "summary",
        "--files-report",
        "-p",
        "Smoke test",
        "--file",
        "smoke.rs",
    ]))?);
    print!("{dry_run}");
    for expected in [format!("Alcuin ({version})"), "smoke.rs".to_owned()] {
        if !dry_run.contains(&expected) {
            return Err(format!("the dry run does not mention {expected:?}"));
        }
    }

    let replies = handshake(command(&mcp), &home.join("mcp.jsonl"))?;
    let server = format!(r#""serverInfo":{{"name":"alcuin-mcp","version":"{version}"}}"#);
    if !replies.contains(&server) {
        return Err(format!("alcuin-mcp did not answer with {server}"));
    }
    for tool in TOOLS {
        if !replies.contains(&format!(r#"{{"name":"{tool}""#)) {
            return Err(format!("alcuin-mcp does not list the {tool} tool"));
        }
    }
    // Kept on failure for inspection, as the error says nothing about its contents.
    let _ = fs::remove_dir_all(&home);
    Ok(())
}

/// A new, empty folder under the system temporary folder.
fn scratch_home() -> Result<PathBuf, String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let home = std::env::temp_dir().join(format!("alcuin-smoke-{}-{nanos}", std::process::id()));
    fs::create_dir(&home).map_err(|e| format!("{}: {e}", home.display()))?;
    Ok(home)
}

/// Runs `command` to completion and fails unless it exits 0, printing what it wrote to stdout
/// first so a failure shows it.
fn run(command: &mut Command) -> Result<Output, String> {
    let output = command
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("{:?}: {e}", command.get_program()))?;
    if !output.status.success() {
        print!("{}", stdout(&output));
        return Err(format!(
            "{:?} {:?} exited with {}",
            command.get_program(),
            command.get_args().collect::<Vec<_>>(),
            output.status
        ));
    }
    Ok(output)
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Sends the MCP handshake and tool listing, closes stdin and returns what the server wrote to
/// `out` before it exited. Its stdout goes to that file, not a pipe, so a process it leaves
/// running cannot hold the step open. A server still running after `MCP_TIMEOUT` is killed and
/// fails.
fn handshake(mut command: Command, out: &Path) -> Result<String, String> {
    let file = fs::File::create(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(file)
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("alcuin-mcp: {e}"))?;
    let deadline = Instant::now() + MCP_TIMEOUT;
    // The input is far smaller than a pipe's buffer, so this write cannot wait on the server.
    let written = child
        .stdin
        .take()
        .ok_or("alcuin-mcp has no stdin")?
        .write_all(MCP_INPUT.as_bytes());
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "alcuin-mcp still running after {} s",
                MCP_TIMEOUT.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(50));
    };
    written.map_err(|e| format!("writing to alcuin-mcp: {e}"))?;
    if !status.success() {
        return Err(format!("alcuin-mcp exited with {status}"));
    }
    fs::read_to_string(out).map_err(|e| format!("{}: {e}", out.display()))
}

#[cfg(test)]
mod tool_list {
    /// Tool names from a pretty-printed list whose entries start `<indent>"name": "`.
    fn names(text: &str, indent: &str) -> Vec<String> {
        let prefix = format!("{indent}\"name\": \"");
        let mut names = text
            .lines()
            .filter_map(|line| line.strip_prefix(&prefix)?.split('"').next())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    /// The smoke test checks every tool the server lists and the bundle declares, so a new tool
    /// cannot ship without its check.
    #[test]
    fn checks_every_tool_the_server_lists_and_the_bundle_declares() {
        let mut checked = super::TOOLS.map(str::to_owned).to_vec();
        checked.sort();
        let listed = names(include_str!("../../../fixtures/mcp-tools.json"), "      ");
        let declared = names(include_str!("../../mcpb/manifest.json"), "    { ");
        assert_eq!(checked, listed, "fixtures/mcp-tools.json");
        assert_eq!(checked, declared, "packaging/mcpb/manifest.json");
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A server that answers and exits, leaving a process that holds its stdout, still passes
    /// at once, as the Bash version did with `timeout 60 alcuin-mcp > mcp.jsonl`. A handshake
    /// that waited for the stdout to close would take the 3 s the leftover process sleeps.
    #[test]
    fn a_process_the_server_leaves_running_does_not_hold_the_handshake() {
        let home = scratch_home().unwrap();
        let server = home.join("PUBLIC-mcp");
        fs::write(
            &server,
            "#!/bin/sh
cat > /dev/null
echo PUBLIC-replies
sleep 3 2> /dev/null &
exit 0
",
        )
        .unwrap();
        fs::set_permissions(&server, fs::Permissions::from_mode(0o700)).unwrap();
        let started = Instant::now();
        let replies = handshake(Command::new(&server), &home.join("mcp.jsonl")).unwrap();
        assert_eq!(
            replies,
            "PUBLIC-replies
"
        );
        assert!(
            started.elapsed() < Duration::from_millis(1_500),
            "{:?}",
            started.elapsed()
        );
        fs::remove_dir_all(&home).unwrap();
    }
}
