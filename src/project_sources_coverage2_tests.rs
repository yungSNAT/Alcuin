use super::*;
use crate::cdp::fake::{self, Reply};

const COMMAND: Duration = Duration::from_secs(1);
const PROJECT_PATH: &str = "/g/g-p-public/project";
const PROJECT_URL: &str = "https://chatgpt.com/g/g-p-public/project?tab=sources";

fn plaintext_cookie_config(directory: &Path) -> Value {
    let path = directory.join("Cookies");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        r"
        CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        INSERT INTO meta VALUES ('version', '23'), ('last_compatible_version', '23');

        CREATE TABLE cookies (
            creation_utc INTEGER NOT NULL DEFAULT 0,
            host_key TEXT NOT NULL,
            top_frame_site_key TEXT NOT NULL DEFAULT '',
            name TEXT NOT NULL,
            value TEXT NOT NULL,
            encrypted_value BLOB NOT NULL DEFAULT X'',
            path TEXT NOT NULL,
            expires_utc INTEGER NOT NULL,
            is_secure INTEGER NOT NULL DEFAULT 1,
            is_httponly INTEGER NOT NULL DEFAULT 1,
            last_access_utc INTEGER NOT NULL DEFAULT 0,
            has_expires INTEGER NOT NULL DEFAULT 1,
            is_persistent INTEGER NOT NULL DEFAULT 1,
            priority INTEGER NOT NULL DEFAULT 1,
            samesite INTEGER NOT NULL DEFAULT -1,
            source_scheme INTEGER NOT NULL DEFAULT 2,
            source_port INTEGER NOT NULL DEFAULT 443,
            last_update_utc INTEGER NOT NULL DEFAULT 0,
            source_type INTEGER NOT NULL DEFAULT 0,
            has_cross_site_ancestor INTEGER NOT NULL DEFAULT 0
        );
        INSERT INTO cookies (host_key, name, value, path, expires_utc)
        VALUES ('.chatgpt.com', '__Secure-next-auth.session-token',
                'PUBLIC-sources-cookie', '/', 15000000000000000);
        ",
    )
    .unwrap();
    json!({
        "manualLogin": false,
        "cookieSync": true,
        "chromeCookiePath": path,
        "cookieNames": ["__Secure-next-auth.session-token"]
    })
}

#[tokio::test]
async fn sources_accept_fresh_plaintext_cookies_and_check_explicit_bootstrap_before_browser_setup()
{
    let home = tempfile::tempdir().unwrap();
    let local_home = home.path().display().to_string();
    let environment = Environment::from([
        ("HOME".into(), local_home.clone()),
        ("XDG_CONFIG_HOME".into(), local_home),
    ]);
    let cancel = CancellationToken::new();
    let config = plaintext_cookie_config(home.path());

    tokio::time::timeout(Duration::from_secs(3), async {
        // Reaching the operation check proves the ordinary read produced a
        // usable bootstrap and satisfied the cookie-sync lifecycle gate.
        let error = run(
            Request {
                operation: "PUBLIC-unknown-operation",
                project_url: PROJECT_URL,
                files: &[],
                config: config.clone(),
            },
            &environment,
            home.path(),
            &cancel,
            None,
            &|_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), "Unknown Project Sources operation");

        let batch = crate::browser_auth::read_ordinary_logged(
            &config,
            &environment,
            home.path(),
            &cancel,
            &|_| {},
        )
        .await
        .unwrap();
        let requirement = batch.requirement();
        let invalid_url = "PUBLIC-not-a-url";
        let validation_error = batch.validate(invalid_url).unwrap_err().to_string();

        for (project_url, required, expected) in [
            (
                invalid_url,
                json!("PUBLIC-mismatched-requirement"),
                "Project Sources requires fresh cookie bootstrap input",
            ),
            (invalid_url, json!(requirement), validation_error.as_str()),
            (
                PROJECT_URL,
                json!(requirement),
                "Project Sources v1 uses local browser automation only. Run it on the signed-in browser host.",
            ),
        ] {
            let mut explicit = config.clone();
            explicit["cookieBootstrapRequired"] = required;
            explicit["remoteChrome"] = json!({});
            let error = run(
                Request {
                    operation: "list",
                    project_url,
                    files: &[],
                    config: explicit,
                },
                &environment,
                home.path(),
                &cancel,
                Some(&batch),
                &|_| {},
            )
            .await
            .unwrap_err();
            assert_eq!(error.to_string(), expected, "{project_url}");
        }
    })
    .await
    .expect("cookie fixture checks timed out");
}

#[tokio::test]
async fn rejected_file_assignment_reports_the_cdp_error_without_sealing_or_confirming() {
    let directory = tempfile::tempdir().unwrap();
    let paths = ["PUBLIC-β.txt", "PUBLIC-note with spaces.txt"].map(|name| {
        let path = directory.path().join(name);
        std::fs::write(&path, b"PUBLIC-upload-content").unwrap();
        path.canonicalize().unwrap()
    });
    let mut files = paths
        .iter()
        .map(|path| UploadFile::open(path).unwrap())
        .collect::<Vec<_>>();
    let cancel = CancellationToken::new();
    let (page, trace) = fake::peer(COMMAND, |request| match request.method.as_str() {
        "Runtime.evaluate" if request.expression().contains(ADD) => {
            Reply::value(json!({"alreadyOpen": true}))
        }
        "Runtime.evaluate" if request.expression().contains(INPUT) => {
            Reply::value(json!({"ok": true}))
        }
        "Runtime.evaluate" if request.dom() != Some("project-sources") => Reply::value(json!(true)),
        "DOM.getDocument" => Reply::Result(json!({"root": {"nodeId": 7}})),
        "DOM.querySelector" => Reply::Result(json!({"nodeId": 11})),
        "DOM.resolveNode" => Reply::Result(json!({"object": {"objectId": "PUBLIC-input"}})),
        "Runtime.callFunctionOn" => Reply::Result(json!({"result": {"value": true}})),
        "Runtime.releaseObject" => Reply::ok(),
        "DOM.setFileInputFiles" => Reply::Error("PUBLIC-file-assignment-rejected".into()),
        other => panic!("unexpected command: {other}"),
    })
    .await;
    let mut reservation = RendererReservation::acquire(
        page.clone(),
        "PUBLIC-sources-owner".into(),
        COMMAND,
        &cancel,
    )
    .await
    .unwrap();
    let mut surface = Surface {
        reservation: &mut reservation,
        path: PROJECT_PATH.into(),
        command: COMMAND,
    };

    let error = tokio::time::timeout(
        Duration::from_millis(1800),
        surface.upload(&mut files, &[], COMMAND, &cancel),
    )
    .await
    .expect("scripted upload timed out")
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "CDP command DOM.setFileInputFiles was refused by the endpoint."
    );

    page.disconnect_and_wait(COMMAND).await.unwrap();
    let trace = tokio::time::timeout(COMMAND, trace).await.unwrap().unwrap();
    let assignments: Vec<_> = trace
        .iter()
        .filter(|command| command["method"] == "DOM.setFileInputFiles")
        .collect();
    assert_eq!(assignments.len(), 1);
    assert_eq!(
        assignments[0]["params"],
        json!({"nodeId": 11, "files": paths})
    );
    assert_eq!(trace.last().unwrap()["method"], "DOM.setFileInputFiles");
}
