//! A genuine Gemini worker crash after its single Send, then public parent recovery
//! from this process against the same live Chrome target and renderer claim.
use super::alternate_support_tests::{
    close_fixture_target, crash_with_manifest, enable_fetch_all, open_fixture_target,
    prepare_parent, run_recovery_with_release, runner_limits, spawn_crashed_worker,
    spawn_static_html_interceptor, target_id_set, wait_for_crash_point, wait_for_targets_baseline,
    worker_endpoint, worker_limits, worker_manifest,
};
use super::*;
use crate::browser_attempt::Provider;

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
    let limits = runner_limits(5);
    let cancel = CancellationToken::new();
    let baseline = target_id_set(browser, limits.command, &cancel).await?;
    let worker = spawn_crashed_worker(
        browser,
        endpoint,
        "browser_runner::gemini_recover_tests::gemini_recovery_worker_helper",
        "ALCUIN_GEMINI_RECOVERY",
        "CRASH",
        crash,
        "Gemini",
        limits.command,
        &cancel,
    )
    .await?;
    let store = &worker.store;
    let parent_id = worker.parent_id.as_str();
    let child_id = worker.child_id.as_str();
    let target_id = worker.target_id.as_str();
    let page = &worker.page;
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
                let committed = request(store, child_id)?;
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
        let (recovered, released) = run_recovery_with_release(
            store,
            parent_id,
            &worker.manifest,
            page,
            browser,
            endpoint,
            &limits,
            &cancel,
            damage == "deadline",
        )
        .await?;
        // Bindings arrive asynchronously; retirement has finished by now.
        tokio::time::sleep(Duration::from_millis(200)).await;
        watcher.abort();
        let replays = replays.load(std::sync::atomic::Ordering::SeqCst);
        let parent = store.require(parent_id)?;
        let child = store.require(child_id)?;
        let targets = target_id_set(browser, limits.command, &cancel).await?;
        if !matches!(damage, "" | "acknowledged") {
            let causes: &[&str] = match damage {
                "temporary" => &["Gemini Temporary Chat is unverified"],
                "claim" => &["Gemini recovery requires the original renderer reservation"],
                // The parent guard's timeout_at, require_session and gemini::recover share
                // guard.deadline(), so whichever check observes expiry first ends the bounded read.
                "deadline" => &[
                    "Public parent recovery deadline reached; original attempt retained",
                    "Parent recovery was cancelled or reached its deadline",
                    "Recovery did not find a completed original Gemini response",
                    "CDP command timed out",
                ],
                "ambiguous" => &["Gemini recovery has no dispatched request"],
                _ => &["Recovered Gemini capture differs from the original answer"],
            };
            let after = store.stored_for_update(child_id)?;
            anyhow::ensure!(
                recovered.as_ref().is_err_and(|error| {
                    let message = format!("{error:#}");
                    causes.iter().any(|cause| message.contains(cause))
                })
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
            String::from_utf8(captured(store, parent_id)?)? == ANSWER
                && fs::read_to_string(worker.manifest["output"].as_str().context("No output")?)?
                    .contains(ANSWER),
            "Recovered capture or output lost the original Gemini answer"
        );
        let targets =
            wait_for_targets_baseline(browser, &baseline, targets, limits.command, &cancel).await?;
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
    let endpoint = worker_endpoint("ALCUIN_GEMINI_RECOVERY")?;
    // The answer deadline outlives the crash point; the witness lifetime covers recovery.
    let limits = worker_limits();
    let cancel = CancellationToken::new();
    let browser = endpoint.connect(limits.command, &cancel).await?;
    let home = root.join("home");
    fs::create_dir_all(&home)?;
    let mut prepared = prepare_parent(&home, PROMPT, json!({"browserProvider":"gemini"}), &cancel)?;
    let link = prepared.coordinator.reserve_first_alternate(
        Provider::Gemini,
        &prepared.environment,
        &cancel,
    )?;
    start_owner(&prepared.store, &link.child_id, &prepared.identity)?;
    let child = prepared.store.require(&link.child_id)?;
    let mut fixture = open_fixture_target(
        &browser,
        Some(&endpoint),
        &child,
        &link.child_id,
        &limits,
        &cancel,
    )
    .await?;
    enable_fetch_all(&fixture.page, limits.command, &cancel).await?;
    let _interceptor = spawn_static_html_interceptor(
        &browser,
        fixture.session.clone(),
        fixture.page.clone(),
        crate::browser_gemini::URL,
        document(&crash),
    );
    let bringup = DirectoryLock::acquire_async(
        &fixture.profile.path().join("oracle-browser-bringup.lock"),
        limits.command,
        &cancel,
    )
    .await?;
    let witness = ExecutionWitness::capture(&prepared.store, &link.child_id, &prepared.identity)?;
    let committed = request(&prepared.store, &link.child_id)?;
    let manifest = worker_manifest(&prepared, &link.child_id, &fixture, &home);
    // A genuine crash: no destructor, witness stop, quarantine or cleanup runs.
    let ready = root.join("ready.json");
    let crash_now = move || -> ! { crash_with_manifest(&ready, &manifest, "Gemini") };
    let delivered = run_owned(
        &prepared.store,
        &link.child_id,
        &witness,
        &committed,
        &mut fixture.target,
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
    let crash_point = wait_for_crash_point(
        &prepared.store,
        &link.child_id,
        &fixture.page,
        "started",
        limits.command,
        &cancel,
    );
    tokio::select! {
        delivered = delivered => {
            anyhow::bail!("PUBLIC worker finished before its crash point: {:?}", delivered.err());
        }
        ready = crash_point => ready?,
    }
    crash_now()
}
