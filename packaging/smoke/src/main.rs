//! Runs a packaged `alcuin` and `alcuin-mcp` in a scratch home: the version, an offline dry run
//! and the MCP handshake with its tool list.
//!
//! Usage: `alcuin-smoke <folder holding both> <version>`

use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, ExitCode, Output, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const TOOLS: [&str; 4] = ["consult", "project_sources", "sessions", "wait"];
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
        return Err(format!("alcuin --version printed {reported:?}, not {version:?}"));
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

    let replies = handshake(command(&mcp))?;
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

/// Runs `command` to completion and fails unless it exits 0.
fn run(command: &mut Command) -> Result<Output, String> {
    let output = command
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("{:?}: {e}", command.get_program()))?;
    if !output.status.success() {
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

/// Sends the MCP handshake and tool listing, closes stdin and returns what the server printed
/// before it exited. A server that is still running after `MCP_TIMEOUT` is killed and fails.
fn handshake(mut command: Command) -> Result<String, String> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("alcuin-mcp: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("alcuin-mcp has no stdout")?;
    let reader = thread::spawn(move || {
        let mut text = String::new();
        stdout.read_to_string(&mut text).map(|_| text)
    });
    let written = child
        .stdin
        .take()
        .ok_or("alcuin-mcp has no stdin")?
        .write_all(MCP_INPUT.as_bytes());
    let deadline = Instant::now() + MCP_TIMEOUT;
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
    reader
        .join()
        .map_err(|_| "the alcuin-mcp reader panicked".to_owned())?
        .map_err(|e| format!("reading alcuin-mcp: {e}"))
}
