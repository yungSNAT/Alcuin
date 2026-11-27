//! A genuine Gemini worker crash after its single Send, then public parent recovery
//! from this process against the same live Chrome target and renderer claim.
use super::grok::tests::{close_fixture_target, target_ids};
use super::*;
use crate::browser_attempt::Provider;
use base64::{Engine as _, engine::general_purpose::STANDARD};

const PROMPT: &str = "PUBLIC Gemini recovery request";
const ANSWER: &str = "PUBLIC original Gemini recovery answer";

/// The public Gemini stand-in. With `crash` "captured" the answer renders at Send;
/// otherwise only the user turn does, and the answer waits for PUBLIC_release().
fn document(crash: &str) -> String {
    let config = json!({"crash":crash,"answer":ANSWER});
    format!(
        r#"<!doctype html><meta http-equiv='Content-Security-Policy' content="default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'none'; form-action 'none'"><style>button,gem-menu-item{{min-width:40px;min-height:24px;display:inline-block}}</style>
<header><button aria-label="Temporary chat">Temporary chat</button></header>
<button aria-label="Open mode picker, currently Auto">Modes</button>
<div id="menu" hidden></div><rich-textarea><div class="ql-editor" contenteditable="true" role="textbox"><p><br></p></div></rich-textarea>
<button aria-label="Send message">Send</button>
<script>
const cfg={config};
window.PUBLIC_sendCount=0;
const temporary=document.querySelector('header button'),picker=document.querySelector('button[aria-label^="Open mode picker"]'),menu=document.querySelector('#menu'),editor=document.querySelector('.ql-editor');
temporary.onclick=()=>temporary.setAttribute('aria-pressed','true');
picker.onclick=()=>{{menu.hidden=!menu.hidden;menu.innerHTML='<gem-menu-item data-mode-id="neon">Deeper analysis 4 Neon</gem-menu-item><gem-menu-item>Thinking level</gem-menu-item>';
menu.children[0].onclick=()=>{{picker.setAttribute('aria-label','Open mode picker, currently Neon Low');setTimeout(()=>{{menu.hidden=true;menu.innerHTML='';}},50);}};
menu.children[1].onclick=()=>{{const level=document.createElement('gem-menu-item');level.innerText='High';level.onclick=()=>{{picker.setAttribute('aria-label','Open mode picker, currently Neon High');menu.hidden=true;}};menu.append(level);}};}};
document.querySelector('button[aria-label="Send message"]').onclick=()=>{{PUBLIC_sendCount++;
const user=document.createElement('user-query');user.innerHTML='<div class="query-text"></div>';
for(const block of editor.children){{const line=document.createElement('p');line.className='query-text-line';line.textContent=block.textContent;user.firstChild.append(line);}}
document.body.append(user);
window.PUBLIC_release=()=>{{const answer=document.createElement('model-response');answer.innerHTML='<message-content><div class="markdown"></div></message-content><button aria-label="Copy response">Copy</button>';
answer.querySelector('.markdown').textContent=cfg.answer;document.body.append(answer);delete window.PUBLIC_release;return true;}};
if(cfg.crash==='captured')PUBLIC_release();}};
</script>"#
    )
}

/// Runs in the disposable native privacy fixture: no account and no provider request.
pub(crate) async fn native_cases(browser: &Cdp, endpoint: &AttachmentEndpoint) -> Result<()> {
    // "acknowledged" stands for a worker that died between its durable acknowledgement and
    // its capture; the rest damage the evidence recovery must refuse.
    for (crash, damage) in [
        ("started", ""),
        ("captured", ""),
        ("started", "acknowledged"),
        ("started", "temporary"),
        ("started", "claim"),
        ("started", "deadline"),
        ("started", "ambiguous"),
        ("captured", "capture"),
    ] {
        recovery_case(browser, endpoint, crash, damage)
            .await
            .with_context(|| format!("PUBLIC Gemini recovery case {crash:?} {damage:?}"))?;
    }
    Ok(())
}

async fn recovery_case(
    browser: &Cdp,
    endpoint: &AttachmentEndpoint,
    crash: &str,
    damage: &str,
) -> Result<()> {
    let limits = RunnerLimits {
        command: Duration::from_secs(5),
        admission: Duration::from_secs(5),
        commit: Duration::from_secs(3),
        answer: Duration::from_secs(5),
        poll: Duration::from_millis(20),
        ..RunnerLimits::default()
    };
    let cancel = CancellationToken::new();
    let baseline = target_ids(
        &browser
            .call("Target.getTargets", json!({}), limits.command, &cancel)
            .await?,
    );
    let root = tempfile::tempdir()?;
    // Windows resolves the worker's startup lock folder from its synthetic USERPROFILE.
    fs::create_dir_all(root.path().join("AppData").join("Local"))?;
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .args([
            "--ignored",
            "--exact",
            "browser_runner::gemini_recover_tests::gemini_recovery_worker_helper",
            "--nocapture",
        ])
        .env_clear()
        .envs(std::env::var_os("LLVM_PROFILE_FILE").map(|value| ("LLVM_PROFILE_FILE", value)))
        .env("ORACLE_DISABLE_NOTIFICATIONS", "1")
        .env("HOME", root.path())
        .env("USERPROFILE", root.path())
        .env("ALCUIN_GEMINI_RECOVERY", root.path())
        .env("ALCUIN_GEMINI_RECOVERY_CRASH", crash)
        .env("ALCUIN_GEMINI_RECOVERY_HOST", &endpoint.host)
        .env("ALCUIN_GEMINI_RECOVERY_PORT", endpoint.port.to_string())
        .env("ALCUIN_GEMINI_RECOVERY_WS", &endpoint.browser_ws_endpoint);
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
    .context("PUBLIC Gemini worker did not crash before its deadline")???;
    anyhow::ensure!(
        output.status.success(),
        "PUBLIC Gemini worker failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest: Value = serde_json::from_slice(&fs::read(root.path().join("ready.json"))?)?;
    let identity: ProcessIdentity = serde_json::from_value(manifest["identity"].clone())?;
    anyhow::ensure!(
        identity.liveness() == crate::process::Liveness::Dead,
        "PUBLIC Gemini worker is not positively dead"
    );
    let store = SessionStore::new(
        Path::new(manifest["home"].as_str().context("No worker home")?).join(".oracle"),
    );
    let parent_id = manifest["parentId"].as_str().context("No parent")?;
    let child_id = manifest["childId"].as_str().context("No child")?;
    let target_id = manifest["targetId"].as_str().context("No target")?;
    let attached = browser
        .call(
            "Target.attachToTarget",
            json!({"targetId":target_id,"flatten":true}),
            limits.command,
            &cancel,
        )
        .await?;
    let page = browser.with_session(
        attached["sessionId"]
            .as_str()
            .context("No recovery observer session")?
            .into(),
    );
    let sends = || page.evaluate("PUBLIC_sendCount", limits.command, &cancel);
    let checked = async {
        anyhow::ensure!(sends().await.context("Send count after the crash")? == 1, "PUBLIC worker did not issue exactly one Send");
        // Count every Send click from here on: recovery must never send, and a recovered
        // target is closed, so its own counter cannot be read afterwards.
        page.call("Runtime.enable", json!({}), limits.command, &cancel)
            .await?;
        page.call(
            "Runtime.addBinding",
            json!({"name":"PUBLIC_sent"}),
            limits.command,
            &cancel,
        )
        .await?;
        page.evaluate("(()=>{const send=document.querySelector('button[aria-label=\"Send message\"]'),click=send.onclick;send.onclick=event=>{PUBLIC_sent('send');return click(event);};return true;})()",limits.command,&cancel).await?;
        let session = page
            .session_id()
            .context("No recovery observer session")?
            .to_owned();
        let mut events = browser.events();
        let replays = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let replayed = replays.clone();
        let watcher = tokio::spawn(async move {
            while let Ok(event) = events.recv().await {
                if event["sessionId"] == session
                    && event["method"] == "Runtime.bindingCalled"
                    && event["params"]["name"] == "PUBLIC_sent"
                {
                    replayed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
            }
        });
        let crashed = store.stored_for_update(child_id)?;
        let expected = if crash == "captured" {
            "acknowledged"
        } else {
            "started"
        };
        anyhow::ensure!(
            crashed["browser"]["runtime"]["geminiCommitReceipt"].is_null() == (crash == "started")
                && crashed["dispatch"]["state"] == expected
                && crashed["capturedAnswer"].is_null() == (crash == "started")
                && store.require(parent_id)?["status"] != "completed",
            "PUBLIC worker crashed at an unexpected point: {crashed}"
        );
        match damage {
            "acknowledged" => {
                // The durable acknowledgement the worker writes after its stable observation.
                let committed = request(&store, child_id)?;
                let owner = serde_json::from_value(
                    crashed["browser"]["runtime"]["submissionOwner"].clone(),
                )?;
                let receipt = crate::browser_gemini::receipt(&owner, &crashed["dispatch"]);
                store.update(child_id, |current| {
                    Ok(Some(json!({
                        "dispatch":merge_object(&current["dispatch"],&json!({"state":"acknowledged"})),
                        "browser":merge_object(&current["browser"],&json!({"runtime":merge_object(&current["browser"]["runtime"],
                            &json!({"geminiCommitReceipt":receipt,"submittedPromptHash":committed.prompt_sha256,
                            "submittedPromptHashFormat":"gemini-request-v1"}))}))})))
                })?;
            }
            "temporary" => {
                page.evaluate(
                    "(document.querySelector('header button').setAttribute('aria-pressed','false'),true)",
                    limits.command,
                    &cancel,
                )
                .await?;
            }
            "claim" => {
                page.evaluate("(()=>{const claim=JSON.parse(sessionStorage.getItem('oracle:target-claim'));claim.submission.id='PUBLIC-other';sessionStorage.setItem('oracle:target-claim',JSON.stringify(claim));return true;})()",limits.command,&cancel).await?;
            }
            "ambiguous" => {
                // A dispatch state the renderer still accepts but no worker journals.
                store.update(child_id, |current| {
                    Ok(Some(
                        json!({"dispatch":merge_object(&current["dispatch"],&json!({"state":"ambiguous"}))}),
                    ))
                })?;
            }
            "capture" => {
                store.update(child_id, |current| {
                    Ok(Some(
                        json!({"capturedAnswer":merge_object(&current["capturedAnswer"],&json!({"sha256":"0".repeat(64)}))}),
                    ))
                })?;
            }
            _ => {}
        }
        let before = store.stored_for_update(child_id)?;
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
        if damage == "deadline" {
            release.abort();
        }
        let profile = ProfileRegistry::open_existing(Path::new(
            manifest["profileDir"].as_str().context("No profile")?,
        ))?;
        let recovered = recover_attached(
            &store,
            parent_id,
            profile,
            browser.clone(),
            endpoint.clone(),
            &limits,
            &cancel,
        )
        .await;
        let released = release.await;
        // Bindings arrive asynchronously; retirement has finished by now.
        tokio::time::sleep(Duration::from_millis(200)).await;
        watcher.abort();
        let replays = replays.load(std::sync::atomic::Ordering::SeqCst);
        let parent = store.require(parent_id)?;
        let child = store.require(child_id)?;
        let mut targets = target_ids(
            &browser
                .call("Target.getTargets", json!({}), limits.command, &cancel)
                .await?,
        );
        if !matches!(damage, "" | "acknowledged") {
            let cause = match damage {
                "temporary" => "Gemini Temporary Chat is unverified",
                "claim" => "Gemini recovery requires the original renderer reservation",
                // The parent guard's own timeout ends the bounded read at the same deadline.
                "deadline" => "Public parent recovery deadline reached; original attempt retained",
                "ambiguous" => "Gemini recovery has no dispatched request",
                _ => "Recovered Gemini capture differs from the original answer",
            };
            let after = store.stored_for_update(child_id)?;
            anyhow::ensure!(
                recovered
                    .as_ref()
                    .is_err_and(|error| format!("{error:#}").contains(cause))
                    && parent["status"] != "completed"
                    && child["capturedAnswer"] == before["capturedAnswer"]
                    && after["dispatch"] == before["dispatch"]
                    && after["browser"]["runtime"]["geminiCommitReceipt"]
                        == before["browser"]["runtime"]["geminiCommitReceipt"]
                    && targets.contains(target_id)
                    && replays == 0
                    && sends().await? == 1,
                "Damaged Gemini evidence was recovered, replayed, changed or closed: {recovered:?}"
            );
            eprintln!(
                "PUBLIC Gemini recovery {damage} refused: {:#}",
                recovered.unwrap_err()
            );
            return Ok(());
        }
        let recovered = recovered.context("Recovery of the crashed Gemini worker")?;
        anyhow::ensure!(
            released?.context("Answer release")? == (crash == "started"),
            "PUBLIC answer release did not match its crash point"
        );
        let runtime = &child["browser"]["runtime"];
        anyhow::ensure!(
            recovered["status"] == "completed"
                && parent["status"] == "completed"
                && child["dispatch"]["state"] == "completed"
                && runtime["geminiCommitReceipt"]["id"] == child["dispatch"]["providerDispatchId"]
                && runtime["geminiCommitReceipt"]["temporaryChat"] == true
                && runtime["acceptedObservation"]["kind"] == "gemini-temporary-answer"
                && runtime["acceptedObservation"]["text"] == ANSWER
                && replays == 0,
            "Recovered Gemini attempt is incomplete or replayed: {child}"
        );
        anyhow::ensure!(
            String::from_utf8(captured(&store, parent_id)?)? == ANSWER
                && fs::read_to_string(manifest["output"].as_str().context("No output")?)?
                    .contains(ANSWER),
            "Recovered capture or output lost the original Gemini answer"
        );
        // Chromium can list a closed target briefly after its retirement is recorded.
        let settle = Instant::now() + Duration::from_secs(5);
        while targets != baseline && Instant::now() < settle {
            tokio::time::sleep(Duration::from_millis(50)).await;
            targets = target_ids(
                &browser
                    .call("Target.getTargets", json!({}), limits.command, &cancel)
                    .await?,
            );
        }
        anyhow::ensure!(
            runtime["targetRetiredAt"].is_string() && targets == baseline,
            "Recovered Gemini target was retained or another target changed: retired {} targets {targets:?} baseline {baseline:?} target {target_id} runtime {runtime}", runtime["targetRetiredAt"]
        );
        eprintln!(
            "PUBLIC Gemini recovery {crash} {damage}: one original Send, durable parent output, exact target absent"
        );
        Ok(())
    }
    .await;
    let closed = if checked.is_err() || !matches!(damage, "" | "acknowledged") {
        // This fixture allocated the target. Its error teardown never runs in production.
        close_fixture_target(browser, target_id, limits.command).await
    } else {
        Ok(())
    };
    checked.and(closed)
}

#[tokio::test]
#[ignore = "Subprocess fixture, invoked only by the native Gemini recovery cases"]
async fn gemini_recovery_worker_helper() {
    let Some(root) = std::env::var_os("ALCUIN_GEMINI_RECOVERY") else {
        return;
    };
    if let Err(error) = recovery_worker(Path::new(&root)).await {
        eprintln!("PUBLIC Gemini recovery worker failed: {error:#}");
        std::process::exit(3);
    }
}

async fn recovery_worker(root: &Path) -> Result<()> {
    let crash = std::env::var("ALCUIN_GEMINI_RECOVERY_CRASH")?;
    let endpoint = AttachmentEndpoint {
        host: std::env::var("ALCUIN_GEMINI_RECOVERY_HOST")?,
        port: std::env::var("ALCUIN_GEMINI_RECOVERY_PORT")?.parse()?,
        browser_ws_endpoint: std::env::var("ALCUIN_GEMINI_RECOVERY_WS")?,
    };
    // The answer deadline outlives the crash point; the witness lifetime covers recovery.
    let limits = RunnerLimits {
        command: Duration::from_secs(5),
        admission: Duration::from_secs(5),
        commit: Duration::from_secs(30),
        answer: Duration::from_secs(30),
        poll: Duration::from_millis(20),
        ..RunnerLimits::default()
    };
    let cancel = CancellationToken::new();
    let browser = endpoint.connect(limits.command, &cancel).await?;
    let home = root.join("home");
    fs::create_dir_all(&home)?;
    let store = SessionStore::new(home.join(".oracle"));
    let output = home.join("PUBLIC-answer.md");
    let environment = crate::config::Environment::from([
        (
            "ORACLE_HOME_DIR".into(),
            store.home().to_string_lossy().into(),
        ),
        (
            (if cfg!(windows) { "USERPROFILE" } else { "HOME" }).into(),
            home.to_string_lossy().into(),
        ),
    ]);
    let resolved = crate::execution::resolve(
        &json!({"prompt":PROMPT,"engine":"browser","model":"gpt-6-pro","writeOutputPath":output,"browserProvider":"gemini"}),
        &json!({"browser":{"manualLogin":true,"cookieSync":false}}),
        &environment,
        &home,
    )?;
    let parent = prepare(&store, &resolved, &home)?;
    let parent_id = parent["id"].as_str().context("No public fixture parent")?;
    let identity = ProcessIdentity::current()?;
    start_owner(&store, parent_id, &identity)?;
    let mut coordinator =
        crate::browser_attempt::Parent::begin(&store, parent_id, &identity, &cancel)?;
    let link = coordinator.reserve_first_alternate(Provider::Gemini, &environment, &cancel)?;
    start_owner(&store, &link.child_id, &identity)?;
    let child = store.require(&link.child_id)?;
    let profile = ProfileRegistry::open(Path::new(
        child["browser"]["config"]["manualLoginProfileDir"]
            .as_str()
            .context("No Gemini profile")?,
    ))?;
    let lease = Arc::new(
        profile
            .acquire_with_limit(&link.child_id, 1, limits.admission, &cancel)
            .await?,
    );
    let _heartbeat = lease.heartbeat();
    let mut target = TargetAcquisition::default()
        .acquire(browser.clone(), lease.clone(), &cancel, limits.command)
        .await?;
    // Production admission records the endpoint that recovery later requires.
    lease.update(&json!({"chromeHost":endpoint.host,"chromePort":endpoint.port}))?;
    let target_id = target.id().to_owned();
    let page = target.page().clone();
    let session = page
        .session_id()
        .context("No owned fixture session")?
        .to_owned();
    let mut events = browser.events();
    page.call(
        "Fetch.enable",
        json!({"patterns":[{"urlPattern":"*"}]}),
        limits.command,
        &cancel,
    )
    .await?;
    let intercepted = page.clone();
    let html = document(&crash);
    let _interceptor = tokio::spawn(async move {
        while let Ok(event) = events.recv().await {
            if event["sessionId"] != session || event["method"] != "Fetch.requestPaused" {
                continue;
            }
            anyhow::ensure!(
                event["params"]["request"]["url"] == crate::browser_gemini::URL,
                "Unexpected fixture network request"
            );
            intercepted.call("Fetch.fulfillRequest",json!({"requestId":event["params"]["requestId"],"responseCode":200,
                "responseHeaders":[{"name":"Content-Type","value":"text/html"}],"body":STANDARD.encode(&html)}),Duration::from_secs(5),&CancellationToken::new()).await?;
        }
        Ok::<_, anyhow::Error>(())
    });
    let bringup = DirectoryLock::acquire_async(
        &profile.path().join("oracle-browser-bringup.lock"),
        limits.command,
        &cancel,
    )
    .await?;
    let witness = ExecutionWitness::capture(&store, &link.child_id, &identity)?;
    let committed = request(&store, &link.child_id)?;
    let manifest = json!({"parentId":parent_id,"childId":link.child_id,"targetId":target_id,
        "profileDir":profile.path(),"home":home,"output":output,"identity":identity});
    // A genuine crash: no destructor, witness stop, quarantine or cleanup runs.
    let ready = root.join("ready.json");
    let crash_now = move || -> ! {
        if let Err(error) = crate::storage::atomic_json(&ready, &manifest) {
            eprintln!("PUBLIC Gemini recovery manifest failed: {error:#}");
            std::process::exit(3);
        }
        std::process::exit(0)
    };
    let delivered = run_owned(
        &store,
        &link.child_id,
        &witness,
        &committed,
        &mut target,
        &endpoint,
        bringup,
        &limits,
        &cancel,
        &CaptureBoundary::ParentAttempt,
        None,
        None,
    );
    if crash == "captured" {
        // Exits after the child capture, before the worker's own clean release.
        let crash_now = crash_now.clone();
        TEST_BEFORE_ATTEMPT_RELEASE.set(Some(Box::new(move || crash_now())));
        let delivered = delivered.await;
        anyhow::bail!(
            "PUBLIC worker passed its crash point: {:?}",
            delivered.err()
        );
    }
    // Exits once the single Send is journalled as started, whilst the answer is held.
    let crash_point = async {
        loop {
            let state = store.stored_for_update(&link.child_id)?["dispatch"]["state"].clone();
            let sent = page
                .evaluate("window.PUBLIC_sendCount??0", limits.command, &cancel)
                .await?
                == 1;
            if sent && state == "started" {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::select! {
        delivered = delivered => {
            anyhow::bail!("PUBLIC worker finished before its crash point: {:?}", delivered.err());
        }
        ready = crash_point => ready?,
    }
    crash_now()
}
