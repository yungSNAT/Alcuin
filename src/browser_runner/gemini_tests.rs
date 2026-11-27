use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::sync::atomic::{AtomicUsize, Ordering};

const PROMPT: &str = "PUBLIC Gemini owned runner request";
const ANSWER: &str = "PUBLIC original Gemini runner answer";

/// A public stand-in for gemini.google.com/app. `case` breaks one guarantee:
/// "focus" leaves Temporary Chat when the composer takes focus, "no-high" offers
/// only a locked High level, "after-send" leaves Temporary Chat once Send is clicked.
fn document(case: &str) -> String {
    let config = json!({"case":case,"answer":ANSWER});
    format!(
        r#"<!doctype html><meta http-equiv='Content-Security-Policy' content="default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'none'; form-action 'none'"><style>button,gem-menu-item{{min-width:40px;min-height:24px;display:inline-block}}</style>
<header><button aria-label="Temporary chat">Temporary chat</button></header>
<button aria-label="Open mode picker, currently Auto">Modes</button>
<div id="menu" hidden></div><rich-textarea><div class="ql-editor" contenteditable="true" role="textbox"><p><br></p></div></rich-textarea>
<button aria-label="Send message">Send</button>
<script>
const cfg={config};
const temporary=document.querySelector('header button'),picker=document.querySelector('button[aria-label^="Open mode picker"]'),menu=document.querySelector('#menu'),editor=document.querySelector('.ql-editor');
temporary.onclick=()=>temporary.setAttribute('aria-pressed','true');
if(cfg.case==='focus')editor.addEventListener('focus',()=>temporary.setAttribute('aria-pressed','false'));
picker.onclick=()=>{{menu.hidden=!menu.hidden;menu.innerHTML='<gem-menu-item data-mode-id="neon">Deeper analysis 4 Neon</gem-menu-item><gem-menu-item>Thinking level</gem-menu-item>';
menu.children[0].onclick=()=>{{picker.setAttribute('aria-label','Open mode picker, currently Neon Low');setTimeout(()=>{{menu.hidden=true;menu.innerHTML='';}},50);}};
menu.children[1].onclick=()=>{{const level=document.createElement('gem-menu-item');level.innerText='High';if(cfg.case==='no-high')level.setAttribute('aria-disabled','true');level.onclick=()=>{{picker.setAttribute('aria-label','Open mode picker, currently Neon High');menu.hidden=true;}};menu.append(level);}};}};
document.querySelector('button[aria-label="Send message"]').onclick=()=>{{PUBLICGeminiSend('send');
const user=document.createElement('user-query');user.innerHTML='<div class="query-text"></div>';
for(const block of editor.children){{const line=document.createElement('p');line.className='query-text-line';line.textContent=block.textContent;user.firstChild.append(line);}}
document.body.append(user);
const answer=document.createElement('model-response');answer.innerHTML='<message-content><div class="markdown"></div></message-content><button aria-label="Copy response">Copy</button>';
answer.querySelector('.markdown').textContent=cfg.answer;document.body.append(answer);
if(cfg.case==='after-send')temporary.setAttribute('aria-pressed','false');}};
</script>"#
    )
}

fn target_ids(rows: &Value) -> std::collections::BTreeSet<String> {
    rows["targetInfos"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row["targetId"].as_str().map(str::to_owned))
        .collect()
}

/// Runs in the disposable native privacy fixture: production Rust and JS drive a
/// public Gemini stand-in in real Chrome, with no account and no provider request.
pub(crate) async fn native_cases(browser: &Cdp, endpoint: &AttachmentEndpoint) -> Result<()> {
    let limits = RunnerLimits {
        command: Duration::from_secs(5),
        admission: Duration::from_secs(5),
        commit: Duration::from_secs(3),
        answer: Duration::from_secs(3),
        poll: Duration::from_millis(20),
        ..RunnerLimits::default()
    };
    let cancel = CancellationToken::new();
    let baseline = browser
        .call("Target.getTargets", json!({}), limits.command, &cancel)
        .await?;
    for case in ["", "focus", "no-high", "after-send"] {
        let home = tempfile::tempdir()?;
        let store = SessionStore::new(home.path().join(".oracle"));
        let output = home.path().join("PUBLIC-answer.md");
        let environment = crate::config::Environment::from([
            (
                "ORACLE_HOME_DIR".into(),
                store.home().to_string_lossy().into(),
            ),
            (
                (if cfg!(windows) { "USERPROFILE" } else { "HOME" }).into(),
                home.path().to_string_lossy().into(),
            ),
        ]);
        let resolved = crate::execution::resolve(
            &json!({"prompt":PROMPT,"engine":"browser","model":"gpt-6-pro","writeOutputPath":output,"browserProvider":"gemini"}),
            &json!({"browser":{"manualLogin":true,"cookieSync":false}}),
            &environment,
            home.path(),
        )?;
        let parent = prepare(&store, &resolved, home.path())?;
        let parent_id = parent["id"].as_str().context("No public fixture parent")?;
        let original_options = parent["options"].clone();
        let identity = ProcessIdentity::current()?;
        start_owner(&store, parent_id, &identity)?;
        let mut coordinator =
            crate::browser_attempt::Parent::begin(&store, parent_id, &identity, &cancel)?;
        let link = coordinator.reserve_first_alternate(Provider::Gemini, &environment, &cancel)?;
        start_owner(&store, &link.child_id, &identity)?;
        let child = store.require(&link.child_id)?;
        let profile_dir = child["browser"]["config"]["manualLoginProfileDir"]
            .as_str()
            .context("No Gemini profile")?;
        anyhow::ensure!(
            profile_dir.ends_with("gemini-browser-profile"),
            "Gemini attempt did not get its own profile: {profile_dir}"
        );
        let profile = ProfileRegistry::open(Path::new(profile_dir))?;
        let lease = Arc::new(
            profile
                .acquire_with_limit(&link.child_id, 1, limits.admission, &cancel)
                .await?,
        );
        let heartbeat = lease.heartbeat();
        let mut target = TargetAcquisition::default()
            .acquire(browser.clone(), lease, &cancel, limits.command)
            .await?;
        let target_id = target.id().to_owned();
        let page = target.page().clone();
        let session = page
            .session_id()
            .context("No owned fixture session")?
            .to_owned();
        let sends = Arc::new(AtomicUsize::new(0));
        let sent = sends.clone();
        let documents = Arc::new(AtomicUsize::new(0));
        let seen = documents.clone();
        let mut events = browser.events();
        page.call("Runtime.enable", json!({}), limits.command, &cancel)
            .await?;
        page.call(
            "Runtime.addBinding",
            json!({"name":"PUBLICGeminiSend"}),
            limits.command,
            &cancel,
        )
        .await?;
        page.call(
            "Fetch.enable",
            json!({"patterns":[{"urlPattern":"*"}]}),
            limits.command,
            &cancel,
        )
        .await?;
        let intercepted = page.clone();
        let html = document(case);
        let interceptor = tokio::spawn(async move {
            while let Ok(event) = events.recv().await {
                if event["sessionId"] != session {
                    continue;
                }
                if event["method"] == "Runtime.bindingCalled"
                    && event["params"]["name"] == "PUBLICGeminiSend"
                {
                    sent.fetch_add(1, Ordering::SeqCst);
                }
                if event["method"] != "Fetch.requestPaused" {
                    continue;
                }
                anyhow::ensure!(
                    event["params"]["request"]["url"] == crate::browser_gemini::URL,
                    "Unexpected fixture network request"
                );
                seen.fetch_add(1, Ordering::SeqCst);
                intercepted.call("Fetch.fulfillRequest",json!({"requestId":event["params"]["requestId"],"responseCode":200,
                    "responseHeaders":[{"name":"Content-Type","value":"text/html"}],"body":STANDARD.encode(&html)}),Duration::from_secs(5),&CancellationToken::new()).await?;
            }
            Ok::<_, anyhow::Error>(())
        });
        let checked = async {
            let bringup = DirectoryLock::acquire_async(
                &profile.path().join("oracle-browser-bringup.lock"),
                limits.command,
                &cancel,
            )
            .await?;
            let witness = ExecutionWitness::capture(&store, &link.child_id, &identity)?;
            let committed = request(&store, &link.child_id)?;
            let delivered = run_owned(
                &store,
                &link.child_id,
                &witness,
                &committed,
                &mut target,
                endpoint,
                bringup,
                &limits,
                &cancel,
                &CaptureBoundary::ParentAttempt,
                None,
                None,
            )
            .await;
            let saved = store.require(&link.child_id)?;
            if !case.is_empty() {
                let error = delivered
                    .as_ref()
                    .err()
                    .map(|error| format!("{error:#}"))
                    .unwrap_or_default();
                let (expected_sends, expected_error) = match case {
                    "focus" => (0, "Gemini composer could not be prepared (focus)"),
                    "no-high" => (
                        0,
                        "Gemini 4 Neon and High thinking could not be verified (select-high)",
                    ),
                    _ => (1, "Gemini Temporary Chat is unverified"),
                };
                anyhow::ensure!(
                    error.contains(expected_error)
                        && sends.load(Ordering::SeqCst) == expected_sends,
                    "Gemini {case} case: {error:?} after {} Sends",
                    sends.load(Ordering::SeqCst)
                );
                anyhow::ensure!(
                    saved["capturedAnswer"].is_null(),
                    "Refused Gemini {case} case produced a capture"
                );
                // Before Send no dispatch is journalled; after it, the dispatch stays started
                // (uncertain) and the worker returns without a second Send.
                let dispatch = saved["dispatch"]["state"].clone();
                anyhow::ensure!(
                    if expected_sends == 0 {
                        dispatch.is_null()
                    } else {
                        dispatch == "started"
                    },
                    "Gemini {case} case left dispatch {dispatch}"
                );
                return Ok::<_, anyhow::Error>(());
            }
            let child_capture = delivered?;
            let runtime = &child_capture["browser"]["runtime"];
            anyhow::ensure!(
                child_capture["status"] == "running"
                    && child_capture["dispatch"]["state"] == "acknowledged"
                    && runtime["provider"] == "gemini"
                    && runtime["modelSelection"] == crate::browser_gemini::model()
                    && runtime["thinkingSelection"] == crate::browser_gemini::thinking()
                    && runtime["geminiCommitReceipt"]["temporaryChat"] == true
                    && runtime["acceptedObservation"]["text"] == ANSWER,
                "Gemini child lacks its provider evidence: {child_capture}"
            );
            coordinator.adopt(&link.child_id, &cancel)?;
            coordinator.complete(&cancel)?;
            crate::browser_attempt::settle_selected_saved(&store, parent_id, &cancel).await?;
            let parent = store.require(parent_id)?;
            anyhow::ensure!(
                parent["status"] == "completed"
                    && parent["recoveryRequired"] == false
                    && parent["options"] == original_options,
                "Public parent completion or original options changed"
            );
            anyhow::ensure!(
                String::from_utf8(captured(&store, parent_id)?)? == ANSWER
                    && fs::read_to_string(&output)?.contains(ANSWER),
                "Public capture/output lost the Gemini answer"
            );
            anyhow::ensure!(
                sends.load(Ordering::SeqCst) == 1 && documents.load(Ordering::SeqCst) == 1,
                "Owned Gemini target repeated navigation or Send"
            );
            anyhow::ensure!(
                target_ids(
                    &browser
                        .call("Target.getTargets", json!({}), limits.command, &cancel)
                        .await?
                ) == target_ids(&baseline),
                "Selected Gemini cleanup retained its target or closed another fixture target"
            );
            eprintln!(
                "PUBLIC native Gemini runner: one Send, parent output durable, exact target absent"
            );
            Ok(())
        }
        .await;
        interceptor.abort();
        let _ = interceptor.await;
        drop(heartbeat);
        // This fixture allocated the target. Its error teardown never runs in production,
        // and a refused close only means the case already closed it.
        if !case.is_empty() || checked.is_err() {
            let _ = browser
                .close_owned_target(&target_id, limits.command, &CancellationToken::new())
                .await;
        }
        checked.with_context(|| format!("PUBLIC Gemini runner case {case:?}"))?;
    }
    Ok(())
}
