//! Shared fixture setup for the native Grok and Gemini runner and recovery cases.
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{collections::BTreeSet, path::PathBuf};

pub(crate) fn runner_limits(answer_secs: u64) -> RunnerLimits {
    RunnerLimits {
        command: Duration::from_secs(5),
        admission: Duration::from_secs(5),
        commit: Duration::from_secs(3),
        answer: Duration::from_secs(answer_secs),
        poll: Duration::from_millis(20),
        ..RunnerLimits::default()
    }
}

pub(crate) fn worker_limits() -> RunnerLimits {
    RunnerLimits {
        command: Duration::from_secs(5),
        admission: Duration::from_secs(5),
        commit: Duration::from_secs(30),
        answer: Duration::from_secs(30),
        poll: Duration::from_millis(20),
        ..RunnerLimits::default()
    }
}

pub(crate) fn fixture_env(home: &Path, store: &SessionStore) -> crate::config::Environment {
    crate::config::Environment::from([
        (
            "ORACLE_HOME_DIR".into(),
            store.home().to_string_lossy().into(),
        ),
        (
            (if cfg!(windows) { "USERPROFILE" } else { "HOME" }).into(),
            home.to_string_lossy().into(),
        ),
    ])
}

pub(crate) struct PreparedParent {
    pub(crate) store: SessionStore,
    pub(crate) output: PathBuf,
    pub(crate) environment: crate::config::Environment,
    pub(crate) parent: Value,
    pub(crate) parent_id: String,
    pub(crate) identity: ProcessIdentity,
    pub(crate) coordinator: crate::browser_attempt::Parent,
}

pub(crate) fn prepare_parent(
    home: &Path,
    prompt: &str,
    extra: Value,
    cancel: &CancellationToken,
) -> Result<PreparedParent> {
    let store = SessionStore::new(home.join(".oracle"));
    let output = home.join("PUBLIC-answer.md");
    let environment = fixture_env(home, &store);
    let mut input = json!({
        "prompt": prompt,
        "engine": "browser",
        "model": "gpt-6-pro",
        "writeOutputPath": output,
    });
    if let Some(map) = extra.as_object() {
        for (key, value) in map {
            input[key] = value.clone();
        }
    }
    let resolved = crate::execution::resolve(
        &input,
        &json!({"browser":{"manualLogin":true,"cookieSync":false}}),
        &environment,
        home,
    )?;
    let parent = prepare(&store, &resolved, home)?;
    let parent_id = parent["id"]
        .as_str()
        .context("No public fixture parent")?
        .to_owned();
    let identity = ProcessIdentity::current()?;
    start_owner(&store, &parent_id, &identity)?;
    let coordinator = crate::browser_attempt::Parent::begin(&store, &parent_id, &identity, cancel)?;
    Ok(PreparedParent {
        store,
        output,
        environment,
        parent,
        parent_id,
        identity,
        coordinator,
    })
}

pub(crate) struct FixtureTarget {
    pub(crate) profile: Arc<ProfileRegistry>,
    pub(crate) heartbeat: crate::browser_profile::LeaseHeartbeat,
    pub(crate) target: OwnedTarget,
    pub(crate) target_id: String,
    pub(crate) page: Cdp,
    pub(crate) session: String,
}

pub(crate) async fn open_fixture_target(
    browser: &Cdp,
    record_endpoint: Option<&AttachmentEndpoint>,
    child: &Value,
    child_id: &str,
    limits: &RunnerLimits,
    cancel: &CancellationToken,
) -> Result<FixtureTarget> {
    let profile_dir = child["browser"]["config"]["manualLoginProfileDir"]
        .as_str()
        .context("No alternate profile")?;
    let profile = ProfileRegistry::open(Path::new(profile_dir))?;
    let lease = Arc::new(
        profile
            .acquire_with_limit(child_id, 1, limits.admission, cancel)
            .await?,
    );
    let heartbeat = lease.heartbeat();
    let target = TargetAcquisition::default()
        .acquire(browser.clone(), lease.clone(), cancel, limits.command)
        .await?;
    if let Some(endpoint) = record_endpoint {
        // Production admission records the endpoint that recovery later requires.
        lease.update(&json!({"chromeHost":endpoint.host,"chromePort":endpoint.port}))?;
    }
    let target_id = target.id().to_owned();
    let page = target.page().clone();
    let session = page
        .session_id()
        .context("No owned fixture session")?
        .to_owned();
    Ok(FixtureTarget {
        profile,
        heartbeat,
        target,
        target_id,
        page,
        session,
    })
}

pub(crate) async fn enable_fetch_all(
    page: &Cdp,
    command: Duration,
    cancel: &CancellationToken,
) -> Result<()> {
    page.call(
        "Fetch.enable",
        json!({"patterns":[{"urlPattern":"*"}]}),
        command,
        cancel,
    )
    .await?;
    Ok(())
}

pub(crate) async fn fulfill_html(page: &Cdp, request_id: &Value, html: &str) -> Result<()> {
    page.call(
        "Fetch.fulfillRequest",
        json!({
            "requestId": request_id,
            "responseCode": 200,
            "responseHeaders": [{"name":"Content-Type","value":"text/html"}],
            "body": STANDARD.encode(html),
        }),
        Duration::from_secs(5),
        &CancellationToken::new(),
    )
    .await?;
    Ok(())
}

pub(crate) fn spawn_static_html_interceptor(
    browser: &Cdp,
    session: String,
    page: Cdp,
    expected_url: &'static str,
    html: String,
) -> tokio::task::JoinHandle<Result<()>> {
    let mut events = browser.events();
    tokio::spawn(async move {
        while let Ok(event) = events.recv().await {
            if event["sessionId"] != session || event["method"] != "Fetch.requestPaused" {
                continue;
            }
            anyhow::ensure!(
                event["params"]["request"]["url"] == expected_url,
                "Unexpected fixture network request"
            );
            fulfill_html(&page, &event["params"]["requestId"], &html).await?;
        }
        Ok(())
    })
}

pub(crate) async fn target_id_set(
    browser: &Cdp,
    command: Duration,
    cancel: &CancellationToken,
) -> Result<BTreeSet<String>> {
    Ok(target_ids(
        &browser
            .call("Target.getTargets", json!({}), command, cancel)
            .await?,
    ))
}

// Pages only: Chrome starts and stops its own service workers and UI targets meanwhile.
pub(crate) fn target_ids(rows: &Value) -> BTreeSet<String> {
    rows["targetInfos"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["type"] == "page")
        .filter_map(|row| row["targetId"].as_str().map(str::to_owned))
        .collect()
}

/// Closes a fixture-allocated target and waits until Chromium stops listing it, so the next
/// case's baseline cannot include a target that is still closing. Fails when absence stays
/// unconfirmed, naming the last close or listing error.
pub(crate) async fn close_fixture_target(
    browser: &Cdp,
    target_id: &str,
    command: Duration,
) -> Result<()> {
    let cancel = CancellationToken::new();
    // A refused close is not final: the case under test may already have closed the target.
    let mut last = browser
        .close_owned_target(target_id, command, &cancel)
        .await
        .err();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match browser
            .call("Target.getTargets", json!({}), command, &cancel)
            .await
        {
            Ok(rows) if !target_ids(&rows).contains(target_id) => return Ok(()),
            Ok(_) => {}
            Err(error) => last = Some(error),
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let unconfirmed = "Fixture target absence was not confirmed within 5 s of closing";
    Err(match last {
        Some(error) => error.context(unconfirmed),
        None => anyhow::anyhow!(unconfirmed),
    })
}

pub(crate) async fn wait_for_targets_baseline(
    browser: &Cdp,
    baseline: &BTreeSet<String>,
    mut targets: BTreeSet<String>,
    command: Duration,
    cancel: &CancellationToken,
) -> Result<BTreeSet<String>> {
    // Chromium can list a closed target briefly after its retirement is recorded.
    let settle = Instant::now() + Duration::from_secs(5);
    while targets != *baseline && Instant::now() < settle {
        tokio::time::sleep(Duration::from_millis(50)).await;
        targets = target_id_set(browser, command, cancel).await?;
    }
    Ok(targets)
}

pub(crate) struct CrashedWorker {
    pub(crate) _root: tempfile::TempDir,
    pub(crate) manifest: Value,
    pub(crate) store: SessionStore,
    pub(crate) parent_id: String,
    pub(crate) child_id: String,
    pub(crate) target_id: String,
    pub(crate) page: Cdp,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn spawn_crashed_worker(
    browser: &Cdp,
    endpoint: &AttachmentEndpoint,
    helper_test: &str,
    env_prefix: &str,
    mode_suffix: &str,
    mode_value: &str,
    label: &str,
    command_timeout: Duration,
    cancel: &CancellationToken,
) -> Result<CrashedWorker> {
    let root = tempfile::tempdir()?;
    // Windows resolves the worker's startup lock folder from its synthetic USERPROFILE.
    fs::create_dir_all(root.path().join("AppData").join("Local"))?;
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .args(["--ignored", "--exact", helper_test, "--nocapture"])
        .env_clear()
        .envs(std::env::var_os("LLVM_PROFILE_FILE").map(|value| ("LLVM_PROFILE_FILE", value)))
        .env("ORACLE_DISABLE_NOTIFICATIONS", "1")
        .env("HOME", root.path())
        .env("USERPROFILE", root.path())
        .env(env_prefix, root.path())
        .env(format!("{env_prefix}_{mode_suffix}"), mode_value)
        .env(format!("{env_prefix}_HOST"), &endpoint.host)
        .env(format!("{env_prefix}_PORT"), endpoint.port.to_string())
        .env(format!("{env_prefix}_WS"), &endpoint.browser_ws_endpoint);
    for key in ["PATH", "SystemRoot", "SYSTEMROOT", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let output = tokio::time::timeout(
        Duration::from_secs(60),
        tokio::task::spawn_blocking(move || command.output()),
    )
    .await
    .with_context(|| format!("PUBLIC {label} worker did not crash before its deadline"))???;
    anyhow::ensure!(
        output.status.success(),
        "PUBLIC {label} worker failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest: Value = serde_json::from_slice(&fs::read(root.path().join("ready.json"))?)?;
    let identity: ProcessIdentity = serde_json::from_value(manifest["identity"].clone())?;
    anyhow::ensure!(
        identity.liveness() == crate::process::Liveness::Dead,
        "PUBLIC {label} worker is not positively dead"
    );
    let store = SessionStore::new(
        Path::new(manifest["home"].as_str().context("No worker home")?).join(".oracle"),
    );
    let parent_id = manifest["parentId"]
        .as_str()
        .context("No parent")?
        .to_owned();
    let child_id = manifest["childId"].as_str().context("No child")?.to_owned();
    let target_id = manifest["targetId"]
        .as_str()
        .context("No target")?
        .to_owned();
    let attached = browser
        .call(
            "Target.attachToTarget",
            json!({"targetId":target_id,"flatten":true}),
            command_timeout,
            cancel,
        )
        .await?;
    let page = browser.with_session(
        attached["sessionId"]
            .as_str()
            .context("No recovery observer session")?
            .into(),
    );
    Ok(CrashedWorker {
        _root: root,
        manifest,
        store,
        parent_id,
        child_id,
        target_id,
        page,
    })
}

pub(crate) fn worker_endpoint(env_prefix: &str) -> Result<AttachmentEndpoint> {
    Ok(AttachmentEndpoint {
        host: std::env::var(format!("{env_prefix}_HOST"))?,
        port: std::env::var(format!("{env_prefix}_PORT"))?.parse()?,
        browser_ws_endpoint: std::env::var(format!("{env_prefix}_WS"))?,
    })
}

pub(crate) fn crash_with_manifest(ready: &Path, manifest: &Value, label: &str) -> ! {
    if let Err(error) = crate::storage::atomic_json(ready, manifest) {
        eprintln!("PUBLIC {label} recovery manifest failed: {error:#}");
        std::process::exit(3);
    }
    std::process::exit(0)
}

/// Builds coherent Gemini dispatch, observation and runtime evidence for unit tests.
pub(crate) fn gemini_evidence(
    req: &BrowserRequest,
    identity: &ProcessIdentity,
    claim_id: &str,
    dispatch_state: &str,
    answer: &str,
    temporary_chat: bool,
) -> (Value, Value, Value) {
    let owner = crate::browser_commit::SubmissionOwner {
        claim_id: claim_id.to_owned(),
        reservation_id: req.reservation_id.clone(),
    };
    let dispatch_id = Uuid::new_v4().to_string();
    let dispatch = json!({
        "state": dispatch_state,
        "reservationId": req.reservation_id,
        "submissionOwner": owner,
        "promptSha256": req.prompt_sha256,
        "providerDispatchId": dispatch_id,
    });
    let observed = json!({
        "kind": "gemini-temporary-answer",
        "id": dispatch_id,
        "temporaryChat": temporary_chat,
        "model": crate::browser_gemini::MODEL,
        "thinking": crate::browser_gemini::THINKING,
        "text": answer,
        "providerText": answer,
    });
    let mut runtime = json!({
        "provider": "gemini",
        "controllerPid": identity.pid,
        "controllerProcessStartedAtMs": identity.started_at_ms,
        "submissionOwner": owner,
        "effectiveModel": "gemini-4-neon",
        "modelSelection": crate::browser_gemini::model(),
        "thinkingSelection": crate::browser_gemini::thinking(),
    });
    if dispatch_state != "started" {
        runtime["geminiCommitReceipt"] = crate::browser_gemini::receipt(&owner, &dispatch);
        runtime["submittedPromptHash"] = json!(req.prompt_sha256);
        runtime["submittedPromptHashFormat"] = json!("gemini-request-v1");
        runtime["acceptedObservation"] = observed.clone();
    }
    (dispatch, observed, runtime)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_recovery_with_release(
    store: &SessionStore,
    parent_id: &str,
    manifest: &Value,
    page: &Cdp,
    browser: &Cdp,
    endpoint: &AttachmentEndpoint,
    limits: &RunnerLimits,
    cancel: &CancellationToken,
    abort_release: bool,
) -> Result<(Result<Value>, Result<Result<Value>, tokio::task::JoinError>)> {
    let releaser = page.clone();
    let release = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        releaser
            .evaluate(
                "window.PUBLIC_release?.()??false",
                Duration::from_secs(5),
                &CancellationToken::new(),
            )
            .await
    });
    if abort_release {
        release.abort();
    }
    let profile = ProfileRegistry::open_existing(Path::new(
        manifest["profileDir"].as_str().context("No profile")?,
    ))?;
    let recovered = recover_attached(
        store,
        parent_id,
        profile,
        browser.clone(),
        endpoint.clone(),
        limits,
        cancel,
    )
    .await;
    let released = release.await;
    Ok((recovered, released))
}

pub(crate) async fn wait_for_crash_point(
    store: &SessionStore,
    child_id: &str,
    page: &Cdp,
    expected_state: &str,
    command: Duration,
    cancel: &CancellationToken,
) -> Result<()> {
    loop {
        let state = store.stored_for_update(child_id)?["dispatch"]["state"].clone();
        let sent = page
            .evaluate("window.PUBLIC_sendCount??0", command, cancel)
            .await?
            == 1;
        if sent && state == expected_state {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub(crate) fn worker_manifest(
    prepared: &PreparedParent,
    child_id: &str,
    fixture: &FixtureTarget,
    home: &Path,
) -> Value {
    json!({
        "parentId": prepared.parent_id,
        "childId": child_id,
        "targetId": fixture.target_id,
        "profileDir": fixture.profile.path(),
        "home": home,
        "output": prepared.output,
        "identity": prepared.identity,
    })
}
