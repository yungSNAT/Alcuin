use alcuin::{
    browser_runner::BrowserRequest,
    config::Environment,
    execution,
    mcp::AlcuinMcp,
    providers::{ApiTransport, Route},
    sessions::SessionStore,
};
use rmcp::model::CallToolResult;
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
    time::{Duration, SystemTime},
};
use tokio_util::sync::CancellationToken;

const ENGINE: &str =
    "fn run() {\n    let status = \"BLOCKED\";\n    // A blank ReviewGroup is refused\n}\n";

fn ground(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_alcuin"))
        .env_clear()
        .env("ORACLE_DISABLE_NOTIFICATIONS", "1")
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("ORACLE_HOME_DIR", home)
        .current_dir(home)
        .arg("ground")
        .args(args)
        .output()
        .unwrap()
}
fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}
/// A store whose `work` folder holds `engine.rs`.
fn fixture() -> (tempfile::TempDir, SessionStore, std::path::PathBuf) {
    let home = tempfile::tempdir().unwrap();
    let work = home.path().join("work");
    fs::create_dir(&work).unwrap();
    fs::write(work.join("engine.rs"), ENGINE).unwrap();
    let store = SessionStore::new(home.path());
    (home, store, work)
}
/// A completed session that names `engine.rs` in `work` but kept no saved request, as an older
/// or remote session can, and saved `answer`.
fn answered(store: &SessionStore, work: &Path, answer: &str) -> String {
    answered_selecting(store, work, "engine.rs", answer)
}
/// As `answered`, for a session started in `cwd` that selected `file`.
fn answered_selecting(store: &SessionStore, cwd: &Path, file: &str, answer: &str) -> String {
    let meta = store
        .create(
            json!({"prompt":"Review the engine for silent refusals.","file":[file],"model":"gpt-6-pro","mode":"api"}),
            cwd,
            None,
        )
        .unwrap();
    let id = meta["id"].as_str().unwrap().to_owned();
    let receipt = alcuin::artifacts::capture(
        store,
        &id,
        "answer.md",
        answer.as_bytes(),
        &tokio_util::sync::CancellationToken::new(),
    )
    .unwrap();
    store
        .update(&id, |_| {
            Ok(Some(json!({"status":"completed","capturedAnswer":receipt})))
        })
        .unwrap();
    id
}

#[test]
fn a_planted_false_citation_exits_1_and_the_corrected_answer_exits_0() {
    let (home, store, work) = fixture();
    let planted = answered(
        &store,
        &work,
        "engine.rs:30 says 'A blank ReviewGroup is refused'.\n",
    );
    let output = ground(home.path(), &[&planted]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output.stderr));
    let stdout = text(&output.stdout);
    assert!(
        stdout.contains("Out of range:\n  answer line 1: engine.rs:30 (file has 4 lines)"),
        "{stdout}"
    );

    // The answer may quote the session's prompt as well as the files it sent.
    let corrected = answered(
        &store,
        &work,
        "engine.rs:3 says 'A blank ReviewGroup is refused', as the brief's 'silent refusals' asks.\n",
    );
    let output = ground(home.path(), &[&corrected, "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report,
        json!({
            "references":[{"answerLine":1,"reference":"engine.rs:3","status":"grounded","fileLines":4}],
            "quotes":[
                {"answerLine":1,"quote":"A blank ReviewGroup is refused","found":true},
                {"answerLine":1,"quote":"silent refusals","found":true}
            ],
            "sentFiles":"current",
            "changedFiles":[],
            "skippedFiles":[]
        })
    );
    let output = ground(home.path(), &[&corrected]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        text(&output.stdout),
        "Grounding: 1 reference (1 grounded, 0 out of range, 0 not in the files sent); 2 quotes (2 found, 0 not found).\nThe session kept no copy of the files it sent, so they were read as they are now.\n"
    );
}

#[test]
fn a_file_changed_after_the_session_is_named_and_checked_as_it_is_now() {
    let (home, store, work) = fixture();
    let id = answered(&store, &work, "engine.rs:5 is new.\n");
    let engine = work.join("engine.rs");
    fs::write(&engine, format!("{ENGINE}// added later\n")).unwrap();
    fs::File::options()
        .write(true)
        .open(&engine)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(60))
        .unwrap();
    let output = ground(home.path(), &[&id, "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["references"][0]["status"], "grounded");
    assert_eq!(report["changedFiles"], json!(["engine.rs"]));
    let output = ground(home.path(), &[&id]);
    assert!(
        text(&output.stdout).contains(
            "Changed since the session was created (checked against their current text):\n  engine.rs"
        ),
        "{}",
        text(&output.stdout)
    );
}

#[test]
fn a_missing_session_or_unreadable_input_fails_with_exit_2() {
    let (home, store, work) = fixture();
    let output = ground(home.path(), &["no-such-session"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(
        text(&output.stderr).contains("Session \"no-such-session\" not found."),
        "{}",
        text(&output.stderr)
    );

    let unanswered = store
        .create(
            json!({"prompt":"PUBLIC pending","file":["engine.rs"],"mode":"api"}),
            &work,
            None,
        )
        .unwrap();
    let output = ground(home.path(), &[unanswered["id"].as_str().unwrap()]);
    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("has no saved answer to check"));

    let id = answered(&store, &work, "nothing cited\n");
    fs::remove_file(work.join("engine.rs")).unwrap();
    let output = ground(home.path(), &[&id]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        text(&output.stderr).contains("Missing file or directory: engine.rs"),
        "{}",
        text(&output.stderr)
    );
}

#[test]
fn another_sessions_staged_uploads_are_not_read_as_this_ones() {
    let (home, store, work) = fixture();
    let prepare = |prompt: &str| {
        let request = resolved(
            &work,
            json!({"prompt":prompt,"engine":"browser","files":["engine.rs"]}),
        );
        alcuin::browser_runner::prepare(&store, &request, &work).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let sent_files = |id: &str| {
        complete(&store, id, "nothing cited\n");
        let output = ground(home.path(), &[id, "--json"]);
        assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["sentFiles"].clone()
    };
    let path = |id: &str| store.directory(id).unwrap().join("browser-request.json");
    let read = |id: &str| serde_json::from_slice::<Value>(&fs::read(path(id)).unwrap()).unwrap();

    // A receipt naming the other session's folder.
    let (own, other) = (prepare("PUBLIC own"), prepare("PUBLIC other"));
    let (mut request, theirs) = (read(&own), read(&other));
    for key in ["stagedUploads", "uploadPaths"] {
        request[key] = theirs[key].clone();
    }
    fs::write(path(&own), serde_json::to_vec(&request).unwrap()).unwrap();
    assert_eq!(sent_files(&own), "current");

    // The session's own folder replaced by a link to the other session's.
    let (own, other) = (prepare("PUBLIC linked"), prepare("PUBLIC target"));
    let staged = |id: &str| {
        std::path::PathBuf::from(read(id)["stagedUploads"]["directory"].as_str().unwrap())
    };
    let (link, target) = (staged(&own), staged(&other));
    fs::remove_dir_all(&link).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link).unwrap();
    #[cfg(windows)]
    {
        // A junction needs no symlink privilege.
        let status = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
    }
    assert_eq!(sent_files(&own), "current");
}

fn mcp(home: &Path, cwd: &Path, env: Environment) -> AlcuinMcp {
    AlcuinMcp {
        store: SessionStore::new(home),
        cwd: cwd.into(),
        env,
    }
}
async fn mcp_ground(server: &AlcuinMcp, id: &str) -> anyhow::Result<CallToolResult> {
    server
        .invoke("ground", json!({"id":id}), &CancellationToken::new())
        .await
}

#[tokio::test]
async fn the_mcp_ground_tool_returns_the_cli_report() {
    let (home, store, work) = fixture();
    let server = mcp(home.path(), home.path(), Environment::new());
    for (answer, code) in [
        ("engine.rs:30 says 'A blank ReviewGroup is refused'.\n", 1),
        ("engine.rs:3 says 'A blank ReviewGroup is refused'.\n", 0),
    ] {
        let id = answered(&store, &work, answer);
        let cli = ground(home.path(), &[&id, "--json"]);
        assert_eq!(cli.status.code(), Some(code), "{}", text(&cli.stderr));
        let mut expected: Value = serde_json::from_slice(&cli.stdout).unwrap();
        expected["sessionId"] = json!(id);
        expected["flagged"] = json!(code == 1);
        // A flagged answer is a result, not a tool error.
        let result = mcp_ground(&server, &id).await.unwrap();
        assert_ne!(result.is_error, Some(true));
        assert_eq!(result.structured_content, Some(expected));
        let report = &result.content[0].as_text().unwrap().text;
        assert_eq!(
            format!("{report}\n"),
            text(&ground(home.path(), &[&id]).stdout)
        );
    }
}

#[tokio::test]
async fn the_mcp_ground_tool_reads_current_files_only_inside_its_directory() {
    let (home, store, work) = fixture();
    let server = mcp(home.path(), &work, Environment::new());
    // Resolved against the session's directory, and inside the server's.
    let nested = work.join("nested");
    fs::create_dir(&nested).unwrap();
    let id = answered_selecting(&store, &nested, "../engine.rs", "engine.rs:3 is cited.\n");
    let result = mcp_ground(&server, &id).await.unwrap();
    assert_eq!(
        result.structured_content.unwrap()["references"][0]["status"],
        "grounded"
    );

    let secret = home.path().join("secret.rs");
    fs::write(&secret, "PUBLIC outside\n").unwrap();
    let opted_in = mcp(
        home.path(),
        &work,
        Environment::from([("ORACLE_MCP_ALLOW_EXTERNAL_INPUT".into(), "1".into())]),
    );
    for file in ["../secret.rs", &secret.to_string_lossy()] {
        let id = answered_selecting(&store, &work, file, "secret.rs:1 is cited.\n");
        let error = mcp_ground(&server, &id).await.unwrap_err().to_string();
        assert!(
            error.starts_with(&format!(
                "MCP file \"{file}\" is outside the working directory"
            )),
            "{error}"
        );
        assert_eq!(ground(home.path(), &[&id]).status.code(), Some(0));
        let result = mcp_ground(&opted_in, &id).await.unwrap();
        assert_eq!(result.structured_content.unwrap()["flagged"], false);
    }

    let elsewhere = answered(&store, home.path(), "nothing cited\n");
    let error = mcp_ground(&server, &elsewhere).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("was started outside this MCP server's working directory"),
        "{error}"
    );
    let error = mcp_ground(&server, "no-such-session").await.unwrap_err();
    assert_eq!(error.to_string(), "Session \"no-such-session\" not found.");
}

/// Marks a session completed with `answer` saved.
fn complete(store: &SessionStore, id: &str, answer: &str) {
    let receipt = alcuin::artifacts::capture(
        store,
        id,
        "answer.md",
        answer.as_bytes(),
        &tokio_util::sync::CancellationToken::new(),
    )
    .unwrap();
    store
        .update(id, |_| {
            Ok(Some(json!({"status":"completed","capturedAnswer":receipt})))
        })
        .unwrap();
}
fn resolved(work: &Path, input: Value) -> execution::ResolvedRequest {
    execution::resolve(&input, &json!({}), &Environment::new(), work).unwrap()
}
const SENT_ANSWER: &str = "engine.rs:4 closes it, notes.md:2 says 'Fabricated notes for tests' and engine.rs:3 quotes 'A blank ReviewGroup is refused'.\n";

#[test]
fn a_session_whose_model_dispatch_no_longer_permits_is_still_checked() {
    let (home, store, work) = fixture();
    let request = resolved(
        &work,
        json!({"prompt":"Review the engine for silent refusals.","engine":"api","files":["engine.rs"]}),
    );
    let route = Route {
        model: "gpt-6-astra".into(),
        base_url: "https://synthetic.example/v1".into(),
        provider_label: "synthetic".into(),
        key_source: "SYNTHETIC_KEY".into(),
        azure_deployment_name: None,
        transport: ApiTransport::Responses,
    };
    let id = execution::prepare(&store, &request, &work, route).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    complete(&store, &id, "engine.rs:4 closes it.\n");
    // As after a release whose model policy no longer names the model this session used.
    let path = store.directory(&id).unwrap().join("dispatch-request.json");
    let mut saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    saved["route"]["model"] = json!("gpt-5-pro");
    fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
    assert!(execution::committed_request(&store, &id).is_err());
    let output = ground(home.path(), &[&id, "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["sentFiles"], "saved");
    assert_eq!(report["references"][0]["status"], "grounded");
}

#[test]
fn an_api_session_is_checked_against_the_request_it_saved_even_with_an_allowed_credential() {
    let (home, store, work) = fixture();
    let token = ["gh", "p_"].concat() + &"PbL7q".repeat(8);
    fs::write(
        work.join("notes.md"),
        format!("key = {token}\nFabricated notes for tests\n"),
    )
    .unwrap();
    let request = resolved(
        &work,
        json!({"prompt":"Review the engine for silent refusals.","engine":"api","files":["engine.rs","notes.md"],"allowSecretFiles":["notes.md"]}),
    );
    let route = Route {
        model: "gpt-6-astra".into(),
        base_url: "https://synthetic.example/v1".into(),
        provider_label: "synthetic".into(),
        key_source: "SYNTHETIC_KEY".into(),
        azure_deployment_name: None,
        transport: ApiTransport::Responses,
    };
    let id = execution::prepare(&store, &request, &work, route).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    complete(&store, &id, SENT_ANSWER);
    // The files on disk no longer hold what was sent; the saved request still does.
    fs::write(work.join("engine.rs"), "fn run() {}\n").unwrap();
    fs::remove_file(work.join("notes.md")).unwrap();
    let output = ground(home.path(), &[&id, "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["sentFiles"], "saved");
    assert_eq!(report["changedFiles"], json!([]));
    let lines: Vec<_> = report["references"]
        .as_array()
        .unwrap()
        .iter()
        .map(|reference| (reference["status"].clone(), reference["fileLines"].clone()))
        .collect();
    assert_eq!(
        lines,
        [
            (json!("grounded"), json!(4)),
            (json!("grounded"), json!(2)),
            (json!("grounded"), json!(4))
        ]
    );
}

#[test]
fn a_browser_session_is_checked_against_its_staged_uploads_until_they_are_removed() {
    // 2 text files go as one text bundle, 1 as its own upload.
    for files in [vec!["engine.rs", "notes.md"], vec!["engine.rs"]] {
        let (home, store, work) = fixture();
        fs::write(
            work.join("notes.md"),
            "PUBLIC heading\nFabricated notes for tests\n",
        )
        .unwrap();
        let request = resolved(
            &work,
            json!({"prompt":"Review the engine for silent refusals.","engine":"browser","files":files}),
        );
        let id = alcuin::browser_runner::prepare(&store, &request, &work).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let answer = if files.len() == 2 {
            SENT_ANSWER
        } else {
            "engine.rs:4 closes it and engine.rs:3 quotes 'A blank ReviewGroup is refused'.\n"
        };
        complete(&store, &id, answer);
        fs::write(work.join("engine.rs"), "fn run() {}\n").unwrap();
        let output = ground(home.path(), &[&id, "--json"]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{files:?}: {}",
            text(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["sentFiles"], "saved", "{files:?}: {report}");

        // A run that closes its tab cleanly removes its staged uploads; then the files are read
        // as they are now, and the output says so.
        let saved: BrowserRequest = serde_json::from_slice(
            &fs::read(store.directory(&id).unwrap().join("browser-request.json")).unwrap(),
        )
        .unwrap();
        saved.staged_uploads.unwrap().cleanup(&store, &id).unwrap();
        let output = ground(home.path(), &[&id]);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{files:?}: {}",
            text(&output.stderr)
        );
        let stdout = text(&output.stdout);
        assert!(
            stdout.contains("The session kept no copy of the files it sent")
                && stdout.contains("answer line 1: engine.rs:4 (file has 1 lines)"),
            "{files:?}: {stdout}"
        );
    }
}
