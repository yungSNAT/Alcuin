use super::*;
use crate::browser_runner::alternate_support_tests::{
    close_fixture_target, enable_fetch_all, fulfill_html, open_fixture_target, prepare_parent,
    runner_limits, target_id_set,
};
use std::sync::atomic::{AtomicUsize, Ordering};

const PROMPT: &str = "PUBLIC Gemini owned runner request";
const ANSWER: &str = "PUBLIC original Gemini runner answer";

/// A public stand-in for gemini.google.com/app. "late" renders Temporary Chat, its
/// pressed state and the Send button after a delay; "active" opens in Temporary Chat.
/// Each other case breaks one guarantee: "focus" leaves Temporary Chat when the composer
/// takes focus, "no-high" offers only a locked High level, "after-send" leaves Temporary
/// Chat once Send is clicked, "no-send" disables Send once the dispatch is journalled and
/// "nav-fail" fails the navigation itself.
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
const header=document.querySelector('header'),temporary=document.querySelector('header button'),picker=document.querySelector('button[aria-label^="Open mode picker"]'),menu=document.querySelector('#menu'),editor=document.querySelector('.ql-editor'),sendButton=document.querySelector('button[aria-label="Send message"]');
const press=()=>temporary.setAttribute('aria-pressed','true');
temporary.onclick=()=>cfg.case==='late'?setTimeout(press,100):press();
if(cfg.case==='active')press();
if(cfg.case==='late'){{temporary.remove();setTimeout(()=>header.append(temporary),150);sendButton.disabled=true;editor.addEventListener('input',()=>setTimeout(()=>{{sendButton.disabled=false;}},100),{{once:true}});}}
if(cfg.case==='no-send'){{const set=Storage.prototype.setItem;Storage.prototype.setItem=function(key,value){{if(key==='oracle:target-claim'&&JSON.parse(value)?.submission?.dispatch==='started')sendButton.disabled=true;return set.call(this,key,value);}};}}
if(cfg.case==='focus')editor.addEventListener('focus',()=>temporary.setAttribute('aria-pressed','false'));
picker.onclick=()=>{{menu.hidden=!menu.hidden;menu.innerHTML='<gem-menu-item data-mode-id="neon">Deeper analysis 4 Neon</gem-menu-item><gem-menu-item>Thinking level</gem-menu-item>';
menu.children[0].onclick=()=>{{picker.setAttribute('aria-label','Open mode picker, currently Neon Low');setTimeout(()=>{{menu.hidden=true;menu.innerHTML='';}},50);}};
menu.children[1].onclick=()=>{{const level=document.createElement('gem-menu-item');level.innerText='High';if(cfg.case==='no-high')level.setAttribute('aria-disabled','true');level.onclick=()=>{{picker.setAttribute('aria-label','Open mode picker, currently Neon High');menu.hidden=true;}};menu.append(level);}};}};
sendButton.onclick=()=>{{PUBLICGeminiSend('send');
const user=document.createElement('user-query');user.innerHTML='<div class="query-text"></div>';
for(const block of editor.children){{const line=document.createElement('p');line.className='query-text-line';line.textContent=block.textContent;user.firstChild.append(line);}}
document.body.append(user);
const answer=document.createElement('model-response');answer.innerHTML='<message-content><div class="markdown"></div></message-content><button aria-label="Copy response">Copy</button>';
answer.querySelector('.markdown').textContent=cfg.answer;document.body.append(answer);
if(cfg.case==='after-send')temporary.setAttribute('aria-pressed','false');}};
</script>"#
    )
}

fn refused(case: &str) -> bool {
    !matches!(case, "" | "late" | "active")
}

/// Runs in the disposable native privacy fixture: production Rust and JS drive a
/// public Gemini stand-in in real Chrome, with no account and no provider request.
pub(crate) async fn native_cases(browser: &Cdp, endpoint: &AttachmentEndpoint) -> Result<()> {
    let limits = runner_limits(3);
    let cancel = CancellationToken::new();
    let baseline = target_id_set(browser, limits.command, &cancel).await?;
    for case in [
        "",
        "late",
        "active",
        "focus",
        "no-high",
        "after-send",
        "no-send",
        "nav-fail",
    ] {
        let home = tempfile::tempdir()?;
        let mut prepared = prepare_parent(
            home.path(),
            PROMPT,
            json!({"browserProvider":"gemini"}),
            &cancel,
        )?;
        let original_options = prepared.parent["options"].clone();
        let link = prepared.coordinator.reserve_first_alternate(
            Provider::Gemini,
            &prepared.environment,
            &cancel,
        )?;
        start_owner(&prepared.store, &link.child_id, &prepared.identity)?;
        let child = prepared.store.require(&link.child_id)?;
        let profile_dir = child["browser"]["config"]["manualLoginProfileDir"]
            .as_str()
            .context("No Gemini profile")?;
        anyhow::ensure!(
            profile_dir.ends_with("gemini-browser-profile"),
            "Gemini attempt did not get its own profile: {profile_dir}"
        );
        let mut fixture =
            open_fixture_target(browser, None, &child, &link.child_id, &limits, &cancel).await?;
        let target_id = fixture.target_id.clone();
        let page = fixture.page.clone();
        let session = fixture.session.clone();
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
        enable_fetch_all(&page, limits.command, &cancel).await?;
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
                if case == "nav-fail" {
                    intercepted.call("Fetch.failRequest",json!({"requestId":event["params"]["requestId"],"errorReason":"Failed"}),
                        Duration::from_secs(5),&CancellationToken::new()).await?;
                    continue;
                }
                fulfill_html(&intercepted, &event["params"]["requestId"], &html).await?;
            }
            Ok::<_, anyhow::Error>(())
        });
        let store = &prepared.store;
        let parent_id = prepared.parent_id.as_str();
        let checked = async {
            let bringup = DirectoryLock::acquire_async(
                &fixture.profile.path().join("oracle-browser-bringup.lock"),
                limits.command,
                &cancel,
            )
            .await?;
            let witness = ExecutionWitness::capture(store, &link.child_id, &prepared.identity)?;
            let committed = request(store, &link.child_id)?;
            // A startup lock defers retirement reconciliation until after the picker.
            let startup = if case == "late" {
                Some(
                    DirectoryLock::acquire_async(
                        &home.path().join("PUBLIC-startup.lock"),
                        limits.command,
                        &cancel,
                    )
                    .await?,
                )
            } else {
                None
            };
            let delivered = run_owned(
                store,
                &link.child_id,
                &witness,
                &committed,
                &mut fixture.target,
                endpoint,
                bringup,
                &limits,
                &cancel,
                &CaptureBoundary::ParentAttempt,
                None,
                startup,
            )
            .await;
            let saved = store.require(&link.child_id)?;
            let refusal = match case {
                "focus" => Some((0, "Gemini composer could not be prepared (focus)", false)),
                "no-high" => Some((
                    0,
                    "Gemini 4 Neon and High thinking could not be verified (select-high)",
                    false,
                )),
                "after-send" => Some((1, "Gemini Temporary Chat is unverified", true)),
                "no-send" => Some((
                    0,
                    "Gemini Send could not be verified; retain the original target without replay",
                    true,
                )),
                "nav-fail" => Some((0, "Gemini navigation failed", false)),
                _ => None,
            };
            if let Some((expected_sends, expected_error, journalled)) = refusal {
                let error = delivered
                    .as_ref()
                    .err()
                    .map(|error| format!("{error:#}"))
                    .unwrap_or_default();
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
                // Before the Send journal no dispatch is recorded; after it, the dispatch
                // stays started (uncertain) and the worker returns without a second Send.
                let dispatch = saved["dispatch"]["state"].clone();
                anyhow::ensure!(
                    if !journalled {
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
            prepared.coordinator.adopt(&link.child_id, &cancel)?;
            prepared.coordinator.complete(&cancel)?;
            crate::browser_attempt::settle_selected_saved(store, parent_id, &cancel).await?;
            let parent = store.require(parent_id)?;
            anyhow::ensure!(
                parent["status"] == "completed"
                    && parent["recoveryRequired"] == false
                    && parent["options"] == original_options,
                "Public parent completion or original options changed"
            );
            anyhow::ensure!(
                String::from_utf8(captured(store, parent_id)?)? == ANSWER
                    && fs::read_to_string(&prepared.output)?.contains(ANSWER),
                "Public capture/output lost the Gemini answer"
            );
            anyhow::ensure!(
                sends.load(Ordering::SeqCst) == 1 && documents.load(Ordering::SeqCst) == 1,
                "Owned Gemini target repeated navigation or Send"
            );
            anyhow::ensure!(
                target_id_set(browser, limits.command, &cancel).await? == baseline,
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
        drop(fixture.heartbeat);
        // This fixture allocated the target. Its error teardown never runs in production,
        // and a refused close only means the case already closed it.
        let closed = if refused(case) || checked.is_err() {
            close_fixture_target(browser, &target_id, limits.command).await
        } else {
            Ok(())
        };
        checked
            .and(closed)
            .with_context(|| format!("PUBLIC Gemini runner case {case:?}"))?;
    }
    Ok(())
}

#[tokio::test]
async fn deliver_refuses_non_gemini_attempt_and_changed_target_before_send() {
    use crate::cdp::fake::{Reply, peer};

    let limits = runner_limits(3);
    let cancel = CancellationToken::new();
    let endpoint = AttachmentEndpoint {
        host: "127.0.0.1".into(),
        port: 9222,
        browser_ws_endpoint: "ws://127.0.0.1:9222/devtools/browser/PUBLIC".into(),
    };
    for (provider, expected) in [
        (Provider::Grok, "Gemini attempt is unavailable"),
        (
            Provider::Gemini,
            "Gemini target or dispatch changed before Send",
        ),
    ] {
        let home = tempfile::tempdir().unwrap();
        let mut prepared = prepare_parent(
            home.path(),
            PROMPT,
            json!({"browserProvider": provider.id()}),
            &cancel,
        )
        .unwrap();
        let link = prepared
            .coordinator
            .reserve_first_alternate(provider, &prepared.environment, &cancel)
            .unwrap();
        start_owner(&prepared.store, &link.child_id, &prepared.identity).unwrap();
        let profile = ProfileRegistry::open(Path::new(
            prepared.store.require(&link.child_id).unwrap()["browser"]["config"]
                ["manualLoginProfileDir"]
                .as_str()
                .unwrap(),
        ))
        .unwrap();
        let lease = Arc::new(
            profile
                .acquire_with_limit(&link.child_id, 1, limits.admission, &cancel)
                .await
                .unwrap(),
        );
        let sends = Arc::new(AtomicUsize::new(0));
        let sent = sends.clone();
        let (browser, _trace) = peer(limits.command, move |req| match req.method.as_str() {
            "Target.attachToTarget" => Reply::Result(json!({"sessionId": "PUBLIC-session"})),
            "Target.getTargetInfo" => Reply::Result(json!({
                "targetInfo": {
                    "targetId": "PUBLIC-target",
                    "type": "page",
                    "url": crate::browser_gemini::URL,
                }
            })),
            "Runtime.evaluate" => {
                let arg = req.arg();
                if arg.get("provider").is_some() {
                    Reply::value(json!(true))
                } else if arg == json!({}) {
                    Reply::value(json!({}))
                } else {
                    match arg["action"].as_str() {
                        Some("type") => Reply::value(json!("entered")),
                        Some("send") => {
                            sent.fetch_add(1, Ordering::SeqCst);
                            Reply::value(json!(true))
                        }
                        _ => Reply::value(json!(true)),
                    }
                }
            }
            _ => Reply::ok(),
        })
        .await;
        let target = OwnedTarget::recover(
            browser,
            lease,
            "PUBLIC-target".into(),
            limits.command,
            &cancel,
        )
        .await
        .unwrap();
        let mut reservation = RendererReservation::acquire_gemini(
            target.page().clone(),
            Uuid::new_v4().to_string(),
            limits.command,
            &cancel,
        )
        .await
        .unwrap();
        let bringup = DirectoryLock::acquire_async(
            &profile.path().join("oracle-browser-bringup.lock"),
            limits.command,
            &cancel,
        )
        .await
        .unwrap();
        let witness =
            ExecutionWitness::capture(&prepared.store, &link.child_id, &prepared.identity).unwrap();
        let committed = request(&prepared.store, &link.child_id).unwrap();
        let error = super::deliver(
            &prepared.store,
            &link.child_id,
            &witness,
            &committed,
            &target,
            &endpoint,
            &mut reservation,
            bringup,
            false,
            &limits,
            &cancel,
        )
        .await
        .err()
        .expect("deliver unexpectedly succeeded");
        assert_eq!(error.to_string(), expected, "{error:#}");
        assert_eq!(sends.load(Ordering::SeqCst), 0);
        assert!(
            prepared.store.require(&link.child_id).unwrap()["dispatch"].is_null(),
            "refused deliver recorded a dispatch"
        );
    }
}
