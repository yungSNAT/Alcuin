use crate::{
    browser_chrome::ChromeConnection,
    browser_dom::UploadFile,
    browser_renderer::RendererReservation,
    browser_target::{AllocationState, TargetAcquisition},
    config::Environment,
    prompts::PromptFile,
    storage::DirectoryLock,
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

const READY: &str = include_str!("../browser-scripts/project-sources-ready.js");
const LIST: &str = include_str!("../browser-scripts/project-sources-list.js");
const TAB: &str = include_str!("../browser-scripts/project-sources-tab.js");
const ADD: &str = include_str!("../browser-scripts/project-sources-add.js");
const INPUT: &str = include_str!("../browser-scripts/project-sources-input.js");
const CONFIRM: &str = include_str!("../browser-scripts/project-sources-confirm.js");
const GUARD: &str = r#"
if(window!==window.top)throw Error('Project Sources requires the top renderer');
const locationUrl=new URL(location.href);
if(locationUrl.protocol!=='https:' || !['chatgpt.com','chat.openai.com'].includes(locationUrl.hostname)
    || locationUrl.pathname.replace(/\/$/,'')!==cfg.projectPath
    || locationUrl.searchParams.get('tab')!=='sources')throw Error('Project Sources navigation changed');
const claim=readOracleTargetClaim();
if(!matchesOracleSubmission(claim,cfg.owner) || claim.submission.state!=='active'
    || claim.submission.dispatch!=='not-started' || claim.inspection)throw Error('Project Sources ownership changed');
"#;

fn expression(body: &str) -> String {
    format!(
        "(cfg)=>{{ /* alcuin-dom:project-sources */ {} {GUARD} return ({body}); }}",
        include_str!("../browser-scripts/ownership-prelude.js")
    )
}

#[cfg(test)]
tokio::task_local! {
    /// A fake browser connection for tests. Unset, `run` configures Chrome itself.
    static TEST_CHROME: std::cell::RefCell<Option<crate::browser_chrome::ConfiguredChrome>>;
}

pub(crate) struct Request<'a> {
    pub operation: &'a str,
    pub project_url: &'a str,
    pub files: &'a [PromptFile],
    pub config: Value,
}

/// Reference buildBrowserConfig parseCookieNames: the option, else ORACLE_BROWSER_COOKIE_NAMES.
/// A given but blank option never falls back to the environment; a blank list keeps the
/// configured names.
pub(crate) fn cookie_names(option: Option<&str>, env: &Environment) -> Option<Value> {
    let raw = option.or_else(|| env.get("ORACLE_BROWSER_COOKIE_NAMES").map(String::as_str))?;
    let names: Vec<_> = raw
        .split(',')
        .map(crate::policy::trim)
        .filter(|name| !name.is_empty())
        .collect();
    (!names.is_empty()).then(|| json!(names))
}
pub(crate) fn browser_config(
    configured: &Value,
    input: &Value,
    env: &Environment,
) -> Result<Value> {
    let mut config = if configured.is_object() {
        configured.clone()
    } else {
        json!({})
    };
    let profile = env
        .get("ORACLE_BROWSER_PROFILE_DIR")
        .map(|v| crate::policy::trim(v))
        .filter(|v| !v.is_empty());
    if crate::browser_profile_copy::configured_source(&config)?.is_some() {
        ensure!(
            profile.is_none(),
            "Copy-profile cannot be combined with a persistent manual-login profile"
        );
    }
    let manual = config["manualLogin"]
        .as_bool()
        .unwrap_or(profile.is_some() || cfg!(windows) && !config["copyProfileSource"].is_string());
    config["manualLogin"] = json!(manual);
    if manual && config["manualLoginProfileDir"].as_str().is_none() {
        config["manualLoginProfileDir"] = json!(profile);
    }
    if !manual && config["copyProfileSource"].is_null() {
        config["manualLoginProfileDir"] = Value::Null;
    }
    config["cookieSync"] = json!(if manual {
        config["manualLoginCookieSync"] == true
    } else {
        config["cookieSync"] == true
    });
    if let Some(value) = input.get("browserKeepBrowser") {
        config["keepBrowser"] = value.clone();
    }
    config["desiredModel"] = Value::Null;
    config["modelStrategy"] = json!("ignore");
    config["researchMode"] = json!("off");
    Ok(config)
}
pub(crate) fn maximum_file_bytes(input: &Value, config: &Value, env: &Environment) -> Result<u64> {
    let configured = input
        .get("maxFileSizeBytes")
        .filter(|v| !v.is_null())
        .map(|v| (v.clone(), "max file size"))
        .or_else(|| {
            env.get("ORACLE_MAX_FILE_SIZE_BYTES")
                .map(|v| crate::policy::trim(v))
                .filter(|v| !v.is_empty())
                .map(|v| (json!(v), "ORACLE_MAX_FILE_SIZE_BYTES"))
        })
        .or_else(|| {
            config
                .get("maxFileSizeBytes")
                .filter(|v| !v.is_null())
                .map(|v| (v.clone(), "config.maxFileSizeBytes"))
        });
    let Some((value, source)) = configured else {
        return Ok(crate::prompts::DEFAULT_MAX_FILE_BYTES);
    };
    let number = if let Some(text) = value.as_str() {
        let text = crate::policy::trim(text);
        let end = text
            .char_indices()
            .find(|(i, c)| !c.is_ascii_digit() && !(*i == 0 && matches!(c, '+' | '-')))
            .map_or(text.len(), |(i, _)| i);
        text[..end].parse::<f64>().ok()
    } else {
        value.as_f64()
    };
    number
        .filter(|n| n.is_finite() && *n > 0.0 && n.fract() == 0.0 && *n <= 9_007_199_254_740_991.0)
        .map(|n| n as u64)
        .with_context(|| format!("{source} must be a positive integer number of bytes."))
}

async fn pause(duration: Duration, cancel: &CancellationToken) -> Result<()> {
    tokio::select! {biased;_=cancel.cancelled()=>bail!("Project Sources operation cancelled"),_=tokio::time::sleep(duration)=>Ok(())}
}

struct Surface<'a> {
    reservation: &'a mut RendererReservation,
    path: String,
    command: Duration,
}
fn decode_sources(value: &Value) -> Result<Vec<Value>> {
    ensure!(
        value["ok"] == true,
        "{}",
        value["error"]
            .as_str()
            .unwrap_or("Unable to read ChatGPT project sources.")
    );
    let rows = value["sources"]
        .as_array()
        .context("Project Sources list is malformed")?;
    for (index, row) in rows.iter().enumerate() {
        let name = row["name"]
            .as_str()
            .context("Project Sources name is malformed")?;
        ensure!(
            (2..=200).contains(&name.encode_utf16().count())
                && name.split_whitespace().collect::<Vec<_>>().join(" ") == name
                && row["index"].as_u64() == Some(index as u64)
                && row["status"] == "unknown",
            "Project Sources row is malformed"
        );
    }
    Ok(rows.clone())
}
impl Surface<'_> {
    async fn eval(
        &mut self,
        body: &str,
        extra: Value,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        self.reservation.assert_active(self.command, cancel).await?;
        let mut cfg = extra;
        cfg["projectPath"] = json!(self.path);
        cfg["owner"] = json!(self.reservation.owner());
        // Sources has its own exact project/owner/no-dispatch boundary. The
        // consultation mutator requires Temporary Chat and cannot run here.
        let result = self
            .reservation
            .evaluate_tracked(&expression(body), &json!([cfg]), self.command, cancel)
            .await;
        if result.is_err() {
            let _ = self
                .reservation
                .quarantine(crate::browser_renderer::CONTROL_ACK_TIMEOUT)
                .await;
        }
        result
    }
    /// Reference clickPoint: locate the control, then click its centre with trusted CDP mouse
    /// events, because some Sources controls activate only on pointer or mouse down.
    async fn click(&mut self, locate: &str, cancel: &CancellationToken) -> Result<bool> {
        let point = self.eval(locate, json!({"click":false}), cancel).await?;
        if point["alreadyOpen"] == true {
            return Ok(true);
        }
        let (true, Some(x), Some(y)) = (
            point["ok"] == true,
            point["x"].as_f64(),
            point["y"].as_f64(),
        ) else {
            return Ok(false);
        };
        self.reservation.assert_active(self.command, cancel).await?;
        for (kind, button) in [
            ("mouseMoved", json!({})),
            ("mousePressed", json!({"button":"left","clickCount":1})),
            ("mouseReleased", json!({"button":"left","clickCount":1})),
        ] {
            let mut event = json!({"type":kind,"x":x,"y":y});
            for (key, value) in button.as_object().into_iter().flatten() {
                event[key] = value.clone();
            }
            self.reservation
                .call_tracked("Input.dispatchMouseEvent", event, self.command, cancel)
                .await?;
        }
        Ok(true)
    }
    async fn list(&mut self, cancel: &CancellationToken) -> Result<Vec<Value>> {
        let value = self.eval(LIST, json!({}), cancel).await?;
        decode_sources(&value)
    }
    async fn settled(&mut self, cancel: &CancellationToken) -> Result<Vec<Value>> {
        let start = Instant::now();
        let deadline = start + self.command.min(Duration::from_secs(30));
        let mut key = None;
        let mut stable = start;
        while Instant::now() < deadline {
            let latest = tokio::time::timeout_at(deadline, self.list(cancel))
                .await
                .context("Project Sources observation exceeded the settlement deadline")??;
            ensure!(
                Instant::now() < deadline,
                "Project Sources list did not settle before timeout"
            );
            let next = latest
                .iter()
                .map(|v| v["name"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>();
            if key.as_ref() != Some(&next) {
                key = Some(next);
                stable = Instant::now();
            }
            if start.elapsed() >= Duration::from_millis(2500)
                && stable.elapsed() >= Duration::from_millis(700)
            {
                return Ok(latest);
            }
            pause(
                Duration::from_millis(300).min(deadline.saturating_duration_since(Instant::now())),
                cancel,
            )
            .await?;
        }
        bail!("Project Sources list did not settle before timeout")
    }
    async fn ready(&mut self, cancel: &CancellationToken) -> Result<()> {
        let deadline = Instant::now() + self.command;
        loop {
            if self.eval(READY, json!({}), cancel).await?["ready"] == true {
                return Ok(());
            }
            ensure!(
                Instant::now() < deadline,
                "Project Sources tab did not become ready before timeout."
            );
            let _ = self.click(TAB, cancel).await?;
            pause(Duration::from_millis(250), cancel).await?;
        }
    }
    async fn upload(
        &mut self,
        files: &mut [UploadFile],
        before: &[Value],
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<Vec<Value>> {
        let mut latest = before.to_vec();
        for batch in files.chunks_mut(10) {
            ensure!(
                self.click(ADD, cancel).await?,
                "Unable to open the Project Sources Add dialog."
            );
            let token = uuid::Uuid::new_v4().to_string();
            let prepare = format!(
                r#"(()=>{{const outcome={INPUT};if(!outcome.ok)return outcome;
                const input=document.querySelector('input[data-oracle-project-sources-input="1"]');
                const dialog=input?.closest('[role="dialog"],dialog');if(!dialog)throw Error('Project Sources Add dialog changed');
                window.__oracleProjectSourcesUpload={{token:cfg.token,input,dialog}};return {{ok:true}};}})()"#
            );
            let deadline = Instant::now() + self.command.min(Duration::from_secs(30));
            loop {
                if self.eval(&prepare, json!({"token":token}), cancel).await?["ok"] == true {
                    break;
                }
                ensure!(
                    Instant::now() < deadline,
                    "Project Sources upload input did not appear."
                );
                pause(Duration::from_millis(200), cancel).await?;
            }
            for file in batch.iter_mut() {
                file.verify()?;
            }
            let page = self.reservation.page().clone();
            let document = page
                .call("DOM.getDocument", json!({"depth":5}), self.command, cancel)
                .await?;
            let node=page.call("DOM.querySelector",json!({"nodeId":document["root"]["nodeId"],"selector":"input[data-oracle-project-sources-input=\"1\"]"}),self.command,cancel).await?;
            let node_id = node["nodeId"]
                .as_u64()
                .filter(|n| *n > 0)
                .context("Unable to locate the Project Sources upload input.")?;
            let remote = page
                .call(
                    "DOM.resolveNode",
                    json!({"nodeId":node_id}),
                    self.command,
                    cancel,
                )
                .await?;
            let object_id = remote["object"]["objectId"]
                .as_str()
                .context("Project Sources input identity missing")?;
            let guard = format!(
                "function(cfg){{ {} {GUARD} const upload=window.__oracleProjectSourcesUpload; return upload?.token===cfg.token && upload.input===this && upload.input.isConnected && upload.dialog.isConnected && upload.dialog.contains(this) && this.type==='file'; }}",
                include_str!("../browser-scripts/ownership-prelude.js")
            );
            let checked=page.call("Runtime.callFunctionOn",json!({"objectId":object_id,"functionDeclaration":guard,"arguments":[{"value":{"owner":self.reservation.owner(),"projectPath":self.path,"token":token}}],"returnByValue":true}),self.command,cancel).await;
            let _ = page
                .call(
                    "Runtime.releaseObject",
                    json!({"objectId":object_id}),
                    self.command,
                    &CancellationToken::new(),
                )
                .await;
            ensure!(
                checked?["result"]["value"] == true,
                "Project Sources upload input ownership changed"
            );
            self.reservation.assert_active(self.command, cancel).await?;
            let paths = batch
                .iter()
                .map(|f| f.manifest()["path"].clone())
                .collect::<Vec<_>>();
            page.call(
                "DOM.setFileInputFiles",
                json!({"nodeId":node_id,"files":paths}),
                self.command,
                cancel,
            )
            .await?;
            for file in batch.iter_mut() {
                file.verify()?;
            }
            let names = batch.iter().map(|f| f.name.clone()).collect::<Vec<_>>();
            let sealed=self.eval(r#"(()=>{const upload=window.__oracleProjectSourcesUpload;
                if(upload?.token!==cfg.token)return false;
                const names=Array.from(upload.input.files,file=>file.name);if(JSON.stringify(names)!==JSON.stringify(cfg.names))return false;
                return true;})()"#,json!({"token":token,"names":names}),cancel).await?;
            ensure!(sealed == true, "Project Sources upload selection changed");
            let baseline = latest.clone();
            latest = self.list(cancel).await?;
            if !uploaded_batch(&baseline, &latest, &names) {
                self.eval(
                    CONFIRM,
                    json!({"click":true,"token":token,"names":names}),
                    cancel,
                )
                .await?;
            }
            let deadline = Instant::now() + timeout.max(Duration::from_secs(30));
            loop {
                latest = self.list(cancel).await?;
                if uploaded_batch(&baseline, &latest, &names) {
                    break;
                }
                ensure!(
                    Instant::now() < deadline,
                    "Timed out waiting for uploaded project sources: {}",
                    names.join(", ")
                );
                pause(Duration::from_millis(500), cancel).await?;
            }
            self.eval("(()=>{if(window.__oracleProjectSourcesUpload?.token===cfg.token)delete window.__oracleProjectSourcesUpload;return true;})()",json!({"token":token}),cancel).await?;
        }
        Ok(latest)
    }
}

fn uploaded_batch(before: &[Value], current: &[Value], names: &[String]) -> bool {
    let mut counts = BTreeMap::<&str, usize>::new();
    for value in current {
        *counts
            .entry(value["name"].as_str().unwrap_or(""))
            .or_default() += 1;
    }
    for value in before {
        let count = counts
            .entry(value["name"].as_str().unwrap_or(""))
            .or_default();
        *count = count.saturating_sub(1);
    }
    for name in names {
        let Some(count) = counts.get_mut(name.as_str()) else {
            return false;
        };
        if *count == 0 {
            return false;
        }
        *count -= 1;
    }
    true
}

pub(crate) async fn run(
    request: Request<'_>,
    environment: &Environment,
    cwd: &Path,
    cancel: &CancellationToken,
    bootstrap: Option<&crate::browser_auth::CookieBootstrap>,
    log: &(dyn Fn(&str) + Sync),
) -> Result<Value> {
    ensure!(
        request.operation != "add",
        "Temporary Chat policy forbids persistent Project Sources uploads"
    );
    let started = Instant::now();
    let config = &request.config;
    ensure!(
        match bootstrap {
            Some(batch) => batch.requirement() == config["cookieBootstrapRequired"],
            None => config["cookieBootstrapRequired"].is_null(),
        },
        "Project Sources requires fresh cookie bootstrap input"
    );
    if let Some(batch) = bootstrap {
        batch.validate(request.project_url)?;
    }
    ensure!(
        !config["remoteChrome"].is_object(),
        "Project Sources v1 uses local browser automation only. Run it on the signed-in browser host."
    );
    ensure!(
        bootstrap.is_some()
            || config["copyProfileSource"].is_string()
            || config["manualLogin"] != true
            || config["cookieSync"] != true,
        "Ordinary Chrome cookie extraction never seeds the persistent manual-login profile; disable manual login or supply explicit cookies"
    );
    // A fresh read for this operation only; it never reaches saved state or another process.
    let extracted = if bootstrap.is_none() && crate::browser_auth::ordinary_sync(config) {
        log(crate::browser_auth::CHROME_COOKIE_SYNC_WARNING);
        let batch = Box::pin(crate::browser_auth::read_ordinary_logged(
            config,
            environment,
            cwd,
            cancel,
            log,
        ))
        .await?;
        batch.validate(request.project_url)?;
        Some(batch)
    } else {
        None
    };
    let bootstrap = bootstrap.or(extracted.as_ref());
    for field in ["cookieSync", "manualLoginCookieSync"] {
        ensure!(
            config[field] != true || bootstrap.is_some() || config["copyProfileSource"].is_string(),
            "Requested browser {field} lifecycle remains gated"
        );
    }
    ensure!(
        config.get("inlineCookies").is_none_or(Value::is_null),
        "Requested browser inlineCookies route remains gated"
    );
    ensure!(
        matches!(request.operation, "list" | "add"),
        "Unknown Project Sources operation"
    );
    ensure!(
        request.operation != "add" || !request.files.is_empty(),
        "Project Sources add requires at least one file."
    );
    let mut files = request
        .files
        .iter()
        .map(|f| UploadFile::open(&f.path))
        .collect::<Result<Vec<_>>>()?;
    let command = Duration::from_millis(config["inputTimeoutMs"].as_u64().unwrap_or(60_000));
    let path = reqwest::Url::parse(request.project_url)?
        .path()
        .trim_end_matches('/')
        .to_owned();
    #[cfg(test)]
    let injected = TEST_CHROME
        .try_with(|chrome| chrome.borrow_mut().take())
        .ok()
        .flatten();
    #[cfg(not(test))]
    let injected = None;
    let configured = match injected {
        Some(configured) => configured,
        None => ChromeConnection::configured(config, environment, cwd, command, cancel).await?,
    };
    let mut connection = configured.connection;
    // Reference projectSourcesRunner hooks, removed before cleanup. A copied profile is never kept.
    let termination = crate::browser_signal::register(crate::browser_signal::Hook::new(
        config["keepBrowser"] == true && !connection.is_copied(),
        config["manualLogin"] == true,
        connection.is_copied(),
        connection.signal_target(),
        // Every caller's log is stderr.
        Box::new(|line| eprintln!("{line}")),
    ));
    let registry = configured.profile;
    let profile_dir = registry.path().to_owned();
    if let Some(line) = profile_line(config, connection.is_fresh(), &profile_dir) {
        log(&line);
    }
    let startup = connection.take_bootstrap_admission();
    ensure!(
        bootstrap.is_none() || connection.owns_process(),
        "Explicit cookie bootstrap requires a newly owned Chrome process"
    );
    let bringup = match connection.take_bootstrap_lock() {
        Some(lock) => lock,
        None => {
            DirectoryLock::acquire_async(
                &registry.path().join("oracle-browser-bringup.lock"),
                Duration::from_millis(config["profileLockTimeoutMs"].as_u64().unwrap_or(300_000)),
                cancel,
            )
            .await?
        }
    };
    let terminate = connection.owns_process() && config["keepBrowser"] != true;
    let acquisition = TargetAcquisition::default();
    let id = format!("project-sources-{}", uuid::Uuid::new_v4());
    let mut lease = None;
    let mut heartbeat = None;
    let mut target = None;
    let mut reservation = None;
    let mut dialogs = None;
    let mut operation=async {
    let acquired = Arc::new(
        registry
            .acquire_with_limit(
                &id,
                crate::browser_health::tab_limit(&config["maxConcurrentTabs"]),
                Duration::from_millis(config["profileLockTimeoutMs"].as_u64().unwrap_or(300_000)),
                cancel,
            )
            .await?,
    );
    heartbeat = Some(acquired.heartbeat());
    lease = Some(acquired.clone());
    acquired.update(
        &json!({"chromeHost":connection.endpoint.host,"chromePort":connection.endpoint.port}),
    )?;
        target=Some(acquisition.acquire(connection.browser.clone(),acquired,cancel,command).await?);
        let owned=target.as_ref().unwrap();
        owned.require(command,cancel).await?;
        dialogs=Some(crate::browser_page::dismiss_dialogs(owned.page(),command,cancel,|line| eprintln!("{line}")).await?);
        let mut applied_cookies=0;
        if let Some(batch)=bootstrap {
            let applied=batch.apply(owned.page(),request.project_url,command,cancel).await?;
            log(&batch.applied_line(applied,config));
            applied_cookies=applied as u64;
        } else if !config["copyProfileSource"].is_string() {
            log(skipped_cookies_line(config));
        }
        // Reference projectSourcesRunner: stale conversation cookies are cleared before navigation.
        for line in crate::browser_auth::clear_stale_conversation_cookies(owned.browser(),owned.page(),&[],command,cancel).await {log(&line);}
        if let Some(startup)=startup {startup.release()?;}
        bringup.release()?;
        // Reference waitForProjectSourcesLogin runs on the ChatGPT home page first.
        let home=owned.page().call("Page.navigate",json!({"url":"https://chatgpt.com/"}),command,cancel).await?;
        ensure!(home.get("errorText").is_none(),"Unable to navigate to ChatGPT before Project Sources");
        crate::browser_page::wait_document_ready(owned.page(),command,cancel).await?;
        crate::browser_page::wait_for_login(owned.page(),config["manualLogin"]==true,
            crate::browser_page::LoginHint{applied_cookies,remote:false},
            crate::browser_page::manual_login_wait(config["timeoutMs"].as_u64(),config["keepBrowser"]==true),
            &profile_dir,command,cancel,log).await?;
        let navigation=owned.page().call("Page.navigate",json!({"url":request.project_url}),command,cancel).await?;
        ensure!(navigation.get("errorText").is_none(),"Unable to navigate to the ChatGPT Project Sources page");
        let deadline=Instant::now()+command;
        loop {
            let ready=owned.page().evaluate_with_arguments("(cfg)=>{const u=new URL(location.href);return document.readyState!=='loading'&&u.protocol==='https:'&&['chatgpt.com','chat.openai.com'].includes(u.hostname)&&u.pathname.replace(/\\/$/,'')===cfg.path&&u.searchParams.get('tab')==='sources';}",&json!([{"path":path}]),command,cancel).await?;
            if ready==true {break;}
            ensure!(Instant::now()<deadline,"Project Sources page did not finish loading before timeout.");
            pause(Duration::from_millis(200),cancel).await?;
        }
        reservation=Some(RendererReservation::acquire(owned.page().clone(),uuid::Uuid::new_v4().to_string(),command,cancel).await?);
        let mut surface=Surface{reservation:reservation.as_mut().unwrap(),path,command};
        surface.ready(cancel).await?;
        let before=surface.settled(cancel).await?;
        let after=if request.operation=="add" {surface.upload(&mut files,&before,Duration::from_millis(config["timeoutMs"].as_u64().unwrap_or(1_200_000)),cancel).await?}else{before.clone()};
        let added=if request.operation=="add" {added_sources(&before,&after)}else{vec![]};
        surface.reservation.release_clean(command).await?;
        Ok::<_,anyhow::Error>(json!({"status":"ok","operation":request.operation,"projectUrl":request.project_url,"dryRun":false,"sourcesBefore":before,"sourcesAfter":after,"plannedUploads":upload_plan(request.files),"added":added,"warnings":[],"tookMs":started.elapsed().as_millis() as u64}))
    }.await;
    drop(termination);
    let cleanup = if operation.is_ok() {
        if let Some(owned) = target.as_mut() {
            if terminate {
                match owned
                    .close_until(std::time::Instant::now() + Duration::from_secs(1))
                    .await
                {
                    Ok(()) => owned.release_retired_lease(),
                    Err(error) => Err(error),
                }
            } else {
                owned.close_preserving_browser(Duration::from_secs(1)).await
            }
        } else {
            Ok(())
        }
    } else {
        let quarantine = match (reservation.as_mut(), target.as_ref()) {
            (Some(reservation), Some(target)) => {
                quarantine_failed_upload(
                    reservation,
                    target,
                    &connection.endpoint,
                    request.project_url,
                )
                .await
            }
            (Some(_), None) => Err(anyhow::anyhow!(
                "Sources cleanup has no original target; retaining recovery"
            )),
            (None, _) => Ok(()),
        };
        let disposition = if bootstrap.is_some()
            && reservation.is_none()
            && (!cancel.is_cancelled()
                || config["closeOwnedTabOnCancel"]
                    .as_bool()
                    .unwrap_or(config["keepBrowser"] != true))
            && let Some(target) = target.as_mut()
        {
            target.retire_unclaimed_blank(Duration::from_secs(1)).await
        } else if target.is_none() && acquisition.state().await == AllocationState::Idle {
            match lease.as_ref() {
                Some(lease) => {
                    lease
                        .release_unissued(std::time::Instant::now() + Duration::from_secs(1))
                        .await
                }
                None => Ok(()),
            }
        } else {
            Ok(())
        };
        quarantine.and(disposition)
    };
    let target_id = target.as_ref().map(|t| t.id().to_owned());
    // Release profile handles before a fresh profile can be removed.
    drop((dialogs, heartbeat, reservation, target, lease, registry));
    if let Ok(value) = &mut operation
        && cleanup.is_ok()
        && terminate
        && !connection.is_copied()
        && let Err(error) = if connection.is_fresh() {
            connection.settle_fresh(Duration::from_secs(5)).await
        } else {
            connection.terminate_owned(Duration::from_secs(1)).await
        }
    {
        value["warnings"]
            .as_array_mut()
            .unwrap()
            .push(json!(format!(
                "Chrome retained because final owned-process termination was unconfirmed: {error:#}"
            )));
    }
    let retained = retain_owned(&mut connection, terminate, &profile_dir, log);
    let disconnect = connection
        .browser
        .disconnect_and_wait(Duration::from_secs(1))
        .await;
    let disposed = connection.dispose_copy().await.map(|_| ());
    let disconnect = disconnect.and(disposed);
    // No consultation session is created, and uncertain uploads are never replayed or closed.
    match (operation, cleanup, retained, disconnect) {
        (Ok(value), Ok(()), Ok(()), Ok(())) => Ok(value),
        (Err(error), Ok(()), Ok(()), Ok(())) => Err(error),
        (result, cleanup, retained, disconnect) => {
            let error = result.err().unwrap_or_else(|| {
                anyhow::anyhow!("Project Sources completed; browser cleanup failed")
            });
            Err(error.context(format!("Project Sources cleanup: target={}, release={cleanup:?}, retained={retained:?}, disconnect={disconnect:?}",target_id.as_deref().unwrap_or("not-allocated"))))
        }
    }
}

/// Lets a Chrome this run launched and did not end outlive the run, and logs where it was
/// left when the run was asked to keep it. A copied profile's Chrome stays owned for disposal.
fn retain_owned(
    connection: &mut ChromeConnection,
    terminate: bool,
    profile_dir: &Path,
    log: &(dyn Fn(&str) + Sync),
) -> Result<()> {
    let owned = connection.owns_process();
    let retained = if owned {
        connection.retain_process()
    } else {
        Ok(())
    };
    // Retention that keeps Chrome hands the process off.
    if owned && !connection.owns_process() && !terminate {
        log(&format!(
            "Chrome left running on port {} with profile {}",
            connection.endpoint.port,
            profile_dir.display()
        ));
    }
    retained
}

async fn quarantine_failed_upload(
    reservation: &mut RendererReservation,
    target: &crate::browser_target::OwnedTarget,
    endpoint: &crate::browser_chrome::AttachmentEndpoint,
    project_url: &str,
) -> Result<()> {
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    let remaining = || {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        ensure!(!remaining.is_zero(), "Sources quarantine deadline reached");
        Ok::<_, anyhow::Error>(remaining)
    };
    let original = reservation.quarantine(remaining()?).await;
    if original.is_ok() || !reservation.page().is_closed() {
        return original;
    }
    target
        .lease()
        .require_unsubmitted_target(target.id(), deadline)
        .await?;
    let browser = endpoint
        .connect(remaining()?, &CancellationToken::new())
        .await?;
    let cleanup = async {
        let page = target
            .attach_unsubmitted_cleanup(&browser, deadline)
            .await?;
        let path = reqwest::Url::parse(project_url)?
            .path()
            .trim_end_matches('/')
            .to_owned();
        reservation
            .quarantine_sources_on(page, &path, remaining()?)
            .await?;
        target
            .lease()
            .require_unsubmitted_target(target.id(), deadline)
            .await
    }
    .await;
    let disconnected = browser
        .disconnect_and_wait(deadline.saturating_duration_since(std::time::Instant::now()))
        .await;
    cleanup.and(disconnected)
}

/// Reference Project Sources profile progress line.
fn profile_line(config: &Value, fresh: bool, profile: &Path) -> Option<String> {
    if config["manualLogin"] == true {
        Some(format!(
            "Manual login mode enabled; reusing persistent profile at {}",
            profile.display()
        ))
    } else if fresh {
        Some(format!(
            "Created temporary Chrome profile at {}",
            profile.display()
        ))
    } else {
        None
    }
}
/// Reference applyProjectSourcesCookies notice when no cookies are synced.
fn skipped_cookies_line(config: &Value) -> &'static str {
    if config["manualLogin"] == true {
        "Skipping Chrome cookie sync (--browser-manual-login enabled); reuse the opened profile after signing in."
    } else {
        "Skipping Chrome cookie copy (disabled by default; use --browser-cookie-sync to opt in)."
    }
}
pub fn normalise_url(raw: &str) -> Result<String> {
    let mut url = reqwest::Url::parse(raw)
        .map_err(|_| anyhow::anyhow!("Invalid ChatGPT project URL: {raw} (Invalid URL)"))?;
    if !matches!(url.host_str(), Some("chatgpt.com" | "chat.openai.com")) {
        bail!("Project Sources require a ChatGPT URL, received: {raw}");
    }
    if !url.path().trim_end_matches('/').ends_with("/project") || url.path().ends_with("//") {
        bail!("Project Sources require a ChatGPT project URL ending in /project, received: {raw}");
    }
    let pairs: Vec<_> = url
        .query_pairs()
        .filter(|(key, _)| key != "tab")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    url.set_query(None);
    url.query_pairs_mut()
        .append_pair("tab", "sources")
        .extend_pairs(pairs);
    Ok(url.into())
}
pub fn upload_plan(files: &[PromptFile]) -> Value {
    json!(files.iter().enumerate().map(|(index,file)|json!({"path":file.path,"displayPath":file.display_path,"name":file.path.file_name().map(|n|n.to_string_lossy().into_owned()).unwrap_or_default(),"sizeBytes":file.size_bytes,"batch":index/10+1})).collect::<Vec<_>>())
}
pub fn added_sources(before: &[Value], after: &[Value]) -> Vec<Value> {
    let mut counts = BTreeMap::new();
    for source in before {
        *counts
            .entry(source["name"].as_str().unwrap_or(""))
            .or_insert(0usize) += 1;
    }
    after
        .iter()
        .filter(|source| {
            let count = counts
                .entry(source["name"].as_str().unwrap_or(""))
                .or_default();
            if *count > 0 {
                *count -= 1;
                false
            } else {
                true
            }
        })
        .cloned()
        .collect()
}

#[cfg(test)]
pub(crate) async fn native_surface_scope_cases(page: &crate::cdp::Cdp) -> Result<()> {
    let cancel = CancellationToken::new();
    let timeout = Duration::from_secs(2);
    // A tab that selects only on pointerdown, which an untrusted element.click() never sends.
    page.evaluate_with_arguments(
        "(cfg)=>{sessionStorage.removeItem('oracle:target-claim');history.replaceState(null,'','/g/g-p-public/project?tab=sources');document.body.innerHTML='<main><button role=\"tab\" aria-selected=\"false\" style=\"width:80px;height:30px\">Sources</button></main>';const tab=document.querySelector('[role=tab]');tab.addEventListener('pointerdown',()=>tab.setAttribute('aria-selected','true'));return true;}",
        &json!([{}]), timeout, &cancel,
    ).await?;
    let mut reservation = RendererReservation::acquire(
        page.clone(),
        "PUBLIC-sources-click-claim".into(),
        timeout,
        &cancel,
    )
    .await?;
    let mut surface = Surface {
        reservation: &mut reservation,
        path: "/g/g-p-public/project".into(),
        command: timeout,
    };
    ensure!(
        surface.click(TAB, &cancel).await?,
        "Sources tab was not located"
    );
    ensure!(
        page.evaluate_with_arguments(
            "(cfg)=>document.querySelector('[role=tab]').getAttribute('aria-selected')",
            &json!([{}]),
            timeout,
            &cancel,
        )
        .await?
            == "true",
        "Sources tab did not receive a trusted pointer press"
    );
    reservation.release_clean(timeout).await?;
    for case in [
        "exact",
        "landing",
        "project",
        "tab",
        "owner",
        "dispatch",
        "inspection",
    ] {
        page.evaluate_with_arguments(
            "(cfg)=>{sessionStorage.removeItem('oracle:target-claim');history.replaceState(null,'','/g/g-p-public/project?tab=sources');document.body.innerHTML='<main>PUBLIC sources</main>';window.PUBLIC_sourcesReads=0;return true;}",
            &json!([{}]), timeout, &cancel,
        ).await?;
        let mut reservation = RendererReservation::acquire(
            page.clone(),
            "PUBLIC-sources-claim".into(),
            timeout,
            &cancel,
        )
        .await?;
        page.evaluate_with_arguments(
            "(cfg)=>{const c=JSON.parse(sessionStorage.getItem('oracle:target-claim'));if(cfg.case==='landing')history.replaceState(null,'','/');if(cfg.case==='project')history.replaceState(null,'','/g/g-p-other/project?tab=sources');if(cfg.case==='tab')history.replaceState(null,'','/g/g-p-public/project?tab=chat');if(cfg.case==='owner')c.id='PUBLIC-foreign';if(cfg.case==='dispatch')c.submission.dispatch='started';if(cfg.case==='inspection')c.inspection={token:'PUBLIC-token',rendererId:'PUBLIC-renderer',parentReservationId:c.submission.id,state:'active'};sessionStorage.setItem('oracle:target-claim',JSON.stringify(c));return true;}",
            &json!([{"case":case}]), timeout, &cancel,
        ).await?;
        let mut surface = Surface {
            reservation: &mut reservation,
            path: "/g/g-p-public/project".into(),
            command: timeout,
        };
        let outcome = surface
            .eval("++window.PUBLIC_sourcesReads", json!({}), &cancel)
            .await;
        ensure!(
            outcome.is_ok() == (case == "exact"),
            "Sources scope {case}: {outcome:?}"
        );
        let reads = page
            .evaluate_with_arguments(
                "(cfg)=>window.PUBLIC_sourcesReads",
                &json!([{}]),
                timeout,
                &cancel,
            )
            .await?;
        ensure!(
            reads == u64::from(case == "exact"),
            "Sources {case} ran an unauthorised body"
        );
    }
    for changing in [false, true] {
        page.evaluate_with_arguments(
            "(cfg)=>{sessionStorage.removeItem('oracle:target-claim');history.replaceState(null,'','/g/g-p-public/project?tab=sources');document.body.innerHTML='<main></main>';window.PUBLIC_listReads=0;if(cfg.changing){const row=document.createElement('button');row.setAttribute('aria-label','PUBLIC-one.txt');const get=row.getAttribute;row.getAttribute=function(name){if(name==='aria-label')return ++window.PUBLIC_listReads%2?'PUBLIC-one.txt':'PUBLIC-two.txt';return get.call(this,name);};document.querySelector('main').append(row);}return true;}",
            &json!([{"changing":changing}]), timeout, &cancel,
        ).await?;
        let mut reservation = RendererReservation::acquire(
            page.clone(),
            "PUBLIC-sources-list-claim".into(),
            timeout,
            &cancel,
        )
        .await?;
        let mut surface = Surface {
            reservation: &mut reservation,
            path: "/g/g-p-public/project".into(),
            command: Duration::from_millis(3300),
        };
        let settled = surface.settled(&cancel).await;
        if changing {
            ensure!(
                settled.is_err(),
                "Changing Sources list reported success: {settled:?}"
            );
            ensure!(
                page.evaluate("window.PUBLIC_listReads", timeout, &cancel)
                    .await?
                    .as_u64()
                    .unwrap_or(0)
                    >= 3,
                "Changing Sources fixture was not observed repeatedly"
            );
        } else {
            ensure!(
                settled? == Vec::<Value>::new(),
                "Settled empty Sources list was rejected"
            );
        }
        reservation.release_clean(timeout).await?;
    }
    page.evaluate_with_arguments(
        "(cfg)=>{sessionStorage.removeItem('oracle:target-claim');return true;}",
        &json!([{}]),
        timeout,
        &cancel,
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn a_kept_owned_chrome_is_logged_with_its_port_and_profile() {
        use crate::browser_chrome::AttachmentEndpoint;
        use std::process::{Command, Stdio};
        use tokio_tungstenite::{WebSocketStream, tungstenite::protocol::Role};
        let profile = Path::new("PUBLIC-profile");
        let home = tempfile::tempdir().unwrap();
        // A copied profile's Chrome is never kept: retention leaves it for disposal.
        for (owned, copied, terminate, logged) in [
            (true, false, false, true),
            (true, false, true, false),
            (false, false, false, false),
            (true, true, false, false),
        ] {
            let (client, _peer) = tokio::io::duplex(64);
            let browser = crate::cdp::Cdp::from_socket(
                WebSocketStream::from_raw_socket(client, Role::Client, None).await,
                Duration::from_secs(1),
            );
            // Any owned child will do: retention only stops tracking it. It has exited before
            // the case starts, so no case can leave a process behind.
            let child = owned.then(|| {
                let mut child = Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "PUBLIC-no-such-test"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap();
                child.wait().unwrap();
                child
            });
            let endpoint = AttachmentEndpoint {
                host: "127.0.0.1".into(),
                port: 9333,
                browser_ws_endpoint: "ws://127.0.0.1:9333/devtools/browser/PUBLIC".into(),
            };
            let copy = copied.then(|| {
                crate::browser_profile_copy::TemporaryProfile::create(home.path()).unwrap()
            });
            let copy_path = copy.as_ref().map(|copy| copy.path().to_owned());
            let mut connection = ChromeConnection::fixture(endpoint, browser, child, copy);
            let lines = std::sync::Mutex::new(Vec::new());
            retain_owned(&mut connection, terminate, profile, &|line| {
                lines.lock().unwrap().push(line.to_owned())
            })
            .unwrap();
            assert_eq!(connection.owns_process(), copied);
            let expected = if logged {
                vec![format!(
                    "Chrome left running on port 9333 with profile {}",
                    profile.display()
                )]
            } else {
                vec![]
            };
            assert_eq!(
                lines.into_inner().unwrap(),
                expected,
                "owned={owned} terminate={terminate}"
            );
            // Dropping the connection disposes of a copied profile.
            drop(connection);
            if let Some(path) = copy_path {
                assert!(!path.exists(), "{}", path.display());
            }
        }
    }
    #[test]
    fn sources_list_rejects_every_malformed_row_without_salvaging_a_partial_list() {
        let good = json!({"name":"PUBLIC.txt","index":0,"status":"unknown"});
        assert_eq!(
            decode_sources(&json!({"ok":true,"sources":[good.clone()]})).unwrap(),
            vec![good.clone()]
        );
        assert!(
            decode_sources(&json!({"ok":true,"sources":[]}))
                .unwrap()
                .is_empty()
        );
        for bad in [
            json!({"name":"PUBLIC-second.txt","index":-1,"status":"unknown"}),
            json!({"name":"PUBLIC-second.txt","index":1.5,"status":"unknown"}),
            json!({"name":"PUBLIC-second.txt","status":"unknown"}),
            json!({"index":1,"status":"unknown"}),
            json!({"name":" PUBLIC.txt","index":1,"status":"unknown"}),
            json!({"name":"x","index":1,"status":"unknown"}),
            json!({"name":"PUBLIC-second.txt","index":1}),
            json!({"name":"PUBLIC-second.txt","index":1,"status":"ready"}),
        ] {
            assert!(
                decode_sources(&json!({"ok":true,"sources":[good.clone(),bad.clone()]})).is_err(),
                "Malformed row accepted: {bad}"
            );
        }
        assert!(decode_sources(&json!({"ok":true,"sources":{}})).is_err());
        let supplied =
            decode_sources(&json!({"ok":false,"error":"PUBLIC read failure"})).unwrap_err();
        assert_eq!(supplied.to_string(), "PUBLIC read failure");
        let fallback = decode_sources(&json!({"ok":false})).unwrap_err();
        assert_eq!(
            fallback.to_string(),
            "Unable to read ChatGPT project sources."
        );
    }
    #[test]
    fn copied_sources_default_to_disposable_login_without_overriding_explicit_manual_choice() {
        let config = json!({"copyProfileSource":"PUBLIC-source"});
        let browser = browser_config(&config, &json!({}), &Environment::new()).unwrap();
        assert_eq!(browser["manualLogin"], false);
        assert!(browser["manualLoginProfileDir"].is_null());
        let mut explicit = config;
        explicit["manualLogin"] = json!(true);
        assert!(browser_config(&explicit, &json!({}), &Environment::new()).is_err());
        let env = Environment::from([(
            "ORACLE_BROWSER_PROFILE_DIR".into(),
            "PUBLIC-persistent".into(),
        )]);
        assert!(
            browser_config(
                &json!({"copyProfileSource":"PUBLIC-source"}),
                &json!({}),
                &env
            )
            .is_err()
        );
        assert!(
            browser_config(
                &json!({"copyProfileSource":"PUBLIC-source","manualLogin":false}),
                &json!({}),
                &env
            )
            .is_err()
        );
    }
    #[tokio::test]
    async fn failed_source_transport_quarantines_only_through_its_exact_saved_authority() {
        use crate::{
            browser_chrome::AttachmentEndpoint, browser_profile::ProfileRegistry,
            browser_target::OwnedTarget, cdp::Cdp,
        };
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::{
            WebSocketStream,
            tungstenite::{Message, protocol::Role},
        };
        for case in [
            "exact",
            "replaced-target",
            "submitted",
            "replaced-owner",
            "wrong-page",
            "changed-renderer",
            "deadline",
        ] {
            let home = tempfile::tempdir().unwrap();
            let profile = ProfileRegistry::open(home.path()).unwrap();
            let token = CancellationToken::new();
            let lease = Arc::new(
                profile
                    .acquire("PUBLIC-source-cleanup", Duration::from_secs(1), &token)
                    .await
                    .unwrap(),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let endpoint = AttachmentEndpoint {
                host: "127.0.0.1".into(),
                port,
                browser_ws_endpoint: format!(
                    "ws://127.0.0.1:{port}/devtools/browser/PUBLIC-cleanup"
                ),
            };
            lease.update(&json!({"chromeHost":"127.0.0.1","chromePort":port,"chromeTargetId":"PUBLIC-source-target"})).unwrap();
            let (client, peer) = tokio::io::duplex(16 * 1024);
            let browser = Cdp::from_socket(
                WebSocketStream::from_raw_socket(client, Role::Client, None).await,
                Duration::from_secs(1),
            );
            let mut peer = WebSocketStream::from_raw_socket(peer, Role::Server, None).await;
            let old_peer = tokio::spawn(async move {
                while let Some(Ok(Message::Text(text))) = peer.next().await {
                    let command: Value = serde_json::from_str(&text).unwrap();
                    let result = match command["method"].as_str().unwrap() {
                        "Target.attachToTarget" => json!({"sessionId":"PUBLIC-old-page"}),
                        "Target.getTargetInfo" => {
                            json!({"targetInfo":{"targetId":"PUBLIC-source-target","type":"page"}})
                        }
                        "Runtime.evaluate" => json!({"result":{"value":true}}),
                        other => panic!("Unexpected original cleanup method: {other}"),
                    };
                    if peer
                        .send(Message::Text(
                            json!({"id":command["id"],"result":result})
                                .to_string()
                                .into(),
                        ))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });
            let target = OwnedTarget::recover(
                browser.clone(),
                lease.clone(),
                "PUBLIC-source-target".into(),
                Duration::from_secs(1),
                &token,
            )
            .await
            .unwrap();
            let mut reservation = RendererReservation::acquire(
                target.page().clone(),
                "PUBLIC-source-claim".into(),
                Duration::from_secs(1),
                &token,
            )
            .await
            .unwrap();
            let owner = reservation.owner().clone();
            browser
                .disconnect_and_wait(Duration::from_secs(1))
                .await
                .unwrap();
            old_peer.await.unwrap();
            if case == "replaced-target" {
                lease
                    .update(&json!({"chromeTargetId":"PUBLIC-replacement"}))
                    .unwrap();
            }
            if case == "submitted" {
                lease.update(&json!({"promptSubmitted":true})).unwrap();
            }
            if case == "replaced-owner" {
                let mut replaced = profile.records().unwrap();
                replaced["leases"][0]["sessionId"] = json!("PUBLIC-new-owner");
                crate::storage::atomic_json(
                    &profile.path().join("oracle-tab-leases.json"),
                    &replaced,
                )
                .unwrap();
            }
            let before = profile.records().unwrap();
            let trace = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
            let observed = trace.clone();
            let fresh_peer = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                while let Some(Ok(Message::Text(text))) = socket.next().await {
                    let command: Value = serde_json::from_str(&text).unwrap();
                    observed.lock().unwrap().push(command.clone());
                    if case == "deadline"
                        && matches!(
                            command["method"].as_str(),
                            Some("Target.attachToTarget" | "Target.getTargetInfo")
                        )
                    {
                        tokio::time::sleep(Duration::from_millis(800)).await;
                    }
                    let result = match command["method"].as_str().unwrap() {
                        "Target.attachToTarget" => {
                            assert_eq!(command["params"]["targetId"], "PUBLIC-source-target");
                            json!({"sessionId":"PUBLIC-fresh-page"})
                        }
                        "Target.getTargetInfo" => {
                            json!({"targetInfo":{"targetId":if case=="wrong-page"{"PUBLIC-foreign"}else{"PUBLIC-source-target"},"type":"page"}})
                        }
                        "Runtime.evaluate" => {
                            let expression = command["params"]["expression"].as_str().unwrap();
                            assert!(
                                expression
                                    .contains(&serde_json::to_string(&owner.claim_id).unwrap())
                            );
                            assert!(
                                expression.contains(
                                    &serde_json::to_string(&owner.reservation_id).unwrap()
                                )
                            );
                            assert!(
                                expression.contains("\"projectPath\":\"/g/g-p-public/project\"")
                            );
                            assert!(expression.contains("\"action\":\"quarantine\""));
                            json!({"result":{"value":case!="changed-renderer"}})
                        }
                        other => panic!("Quarantine attempted an unauthorised method: {other}"),
                    };
                    if socket
                        .send(Message::Text(
                            json!({"id":command["id"],"result":result})
                                .to_string()
                                .into(),
                        ))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });
            let started = std::time::Instant::now();
            let result = quarantine_failed_upload(
                &mut reservation,
                &target,
                &endpoint,
                "https://chatgpt.com/g/g-p-public/project?tab=sources",
            )
            .await;
            if case == "deadline" {
                assert!(
                    started.elapsed() < Duration::from_millis(1500),
                    "Cleanup restarted its deadline"
                );
            }
            fresh_peer.abort();
            let joined = fresh_peer.await;
            assert!(joined.is_ok() || joined.is_err_and(|error| error.is_cancelled()));
            assert_eq!(result.is_ok(), case == "exact", "{case}: {result:?}");
            assert_eq!(
                profile.records().unwrap(),
                before,
                "Cleanup changed native admission"
            );
            assert!(
                !reservation.settled_before_dispatch(),
                "Transport uncertainty was cleared"
            );
            if matches!(case, "replaced-target" | "submitted" | "replaced-owner") {
                assert!(trace.lock().unwrap().is_empty());
            }
            if case == "wrong-page" {
                assert!(
                    trace
                        .lock()
                        .unwrap()
                        .iter()
                        .all(|row| row["method"] != "Runtime.evaluate")
                );
            }
        }
    }
    #[test]
    fn source_progress_lines_use_the_reference_wording() {
        let profile = Path::new("PUBLIC-profile");
        assert_eq!(
            profile_line(&json!({"manualLogin":true}), false, profile).as_deref(),
            Some("Manual login mode enabled; reusing persistent profile at PUBLIC-profile")
        );
        assert_eq!(
            profile_line(&json!({"manualLogin":false}), true, profile).as_deref(),
            Some("Created temporary Chrome profile at PUBLIC-profile")
        );
        assert_eq!(
            profile_line(&json!({"manualLogin":false}), false, profile),
            None
        );
        assert_eq!(
            skipped_cookies_line(&json!({"manualLogin":true})),
            "Skipping Chrome cookie sync (--browser-manual-login enabled); reuse the opened profile after signing in."
        );
        assert_eq!(
            skipped_cookies_line(&json!({"manualLogin":false})),
            "Skipping Chrome cookie copy (disabled by default; use --browser-cookie-sync to opt in)."
        );
    }

    #[cfg(any(windows, target_os = "linux"))]
    #[tokio::test]
    async fn ordinary_source_cookie_sync_logs_the_reference_warning_before_reading_chrome() {
        let home = tempfile::tempdir().unwrap();
        let lines = std::sync::Mutex::new(Vec::<String>::new());
        let result = run(
            Request {
                operation: "list",
                project_url: "https://chatgpt.com/g/g-p-public/project",
                files: &[],
                config: json!({"manualLogin":false,"cookieSync":true,"chromeCookiePath":home.path().join("PUBLIC-missing/Cookies")}),
            },
            &Environment::new(),
            home.path(),
            &CancellationToken::new(),
            None,
            &|line| lines.lock().unwrap().push(line.to_owned()),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(
            lines.lock().unwrap().first().map(String::as_str),
            Some(crate::browser_auth::CHROME_COOKIE_SYNC_WARNING)
        );
    }

    #[test]
    fn every_duplicate_basename_requires_a_new_source_occurrence() {
        let before = vec![json!({"name":"context.md"})];
        let names = vec!["context.md".into(), "context.md".into()];
        assert!(!uploaded_batch(
            &before,
            &[before[0].clone(), before[0].clone()],
            &names
        ));
        assert!(uploaded_batch(
            &before,
            &[before[0].clone(), before[0].clone(), before[0].clone()],
            &names
        ));
        assert!(!uploaded_batch(
            &before,
            &[
                json!({"name":"Context.md"}),
                json!({"name":"Context.md"}),
                json!({"name":"Context.md"})
            ],
            &names
        ));
    }
    #[test]
    fn file_limits_keep_cli_environment_configuration_precedence_and_safe_integer_validation() {
        let env = Environment::from([("ORACLE_MAX_FILE_SIZE_BYTES".into(), " 20bytes ".into())]);
        assert_eq!(
            maximum_file_bytes(&json!({}), &json!({"maxFileSizeBytes":30}), &env).unwrap(),
            20
        );
        assert_eq!(
            maximum_file_bytes(
                &json!({"maxFileSizeBytes":40}),
                &json!({"maxFileSizeBytes":30}),
                &env
            )
            .unwrap(),
            40
        );
        for value in [
            json!(0),
            json!(-1),
            json!(1.5),
            json!(9_007_199_254_740_992u64),
            json!("bad"),
        ] {
            assert!(
                maximum_file_bytes(
                    &json!({}),
                    &json!({"maxFileSizeBytes":value}),
                    &Environment::new()
                )
                .is_err()
            );
        }
        assert_eq!(
            maximum_file_bytes(
                &json!({}),
                &json!({"maxFileSizeBytes":30}),
                &Environment::from([("ORACLE_MAX_FILE_SIZE_BYTES".into(), " ".into())])
            )
            .unwrap(),
            30
        );
    }
    #[test]
    fn source_configuration_ignores_model_selection_and_honours_explicit_manual_profile_choices() {
        let env = Environment::from([(
            "ORACLE_BROWSER_PROFILE_DIR".into(),
            "PUBLIC-env-profile".into(),
        )]);
        let config = browser_config(
            &json!({"manualLogin":false,"manualLoginProfileDir":"PUBLIC-old","cookieSync":true,"desiredModel":"PUBLIC-old-model","researchMode":"deep"}),
            &json!({"browserKeepBrowser":false}),
            &env,
        ).unwrap();
        assert_eq!(config["manualLogin"], false);
        assert_eq!(config["manualLoginProfileDir"], Value::Null);
        assert_eq!(config["cookieSync"], true);
        assert_eq!(config["modelStrategy"], "ignore");
        assert_eq!(config["researchMode"], "off");
        assert_eq!(config["desiredModel"], Value::Null);
        assert_eq!(config["keepBrowser"], false);
    }
    #[test]
    fn a_missing_configuration_is_empty_and_an_environment_profile_backs_manual_login() {
        let env = Environment::from([(
            "ORACLE_BROWSER_PROFILE_DIR".into(),
            " PUBLIC-env-profile ".into(),
        )]);
        let config = browser_config(&Value::Null, &json!({}), &env).unwrap();
        assert_eq!(config["manualLogin"], true);
        assert_eq!(config["manualLoginProfileDir"], "PUBLIC-env-profile");
        assert_eq!(config["cookieSync"], false);
        assert_eq!(config["modelStrategy"], "ignore");
        assert!(config.get("keepBrowser").is_none());
        assert!(!uploaded_batch(&[], &[], &["PUBLIC-absent.txt".into()]));
    }

    type Commands = Arc<std::sync::Mutex<Vec<Value>>>;
    const TARGET: &str = "PUBLIC-sources-target";
    const PROJECT: &str = "https://chatgpt.com/g/g-p-public/project?tab=sources";

    /// An in-memory DevTools peer: `reply` answers each command, and every command is kept.
    async fn fake_devtools(
        mut reply: impl FnMut(&Value) -> Value + Send + 'static,
    ) -> (crate::cdp::Cdp, Commands, tokio::task::JoinHandle<()>) {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::{
            WebSocketStream,
            tungstenite::{Message, protocol::Role},
        };
        let (client, peer) = tokio::io::duplex(1024 * 1024);
        let cdp = crate::cdp::Cdp::from_socket(
            WebSocketStream::from_raw_socket(client, Role::Client, None).await,
            Duration::from_secs(10),
        );
        let mut peer = WebSocketStream::from_raw_socket(peer, Role::Server, None).await;
        let commands = Commands::default();
        let seen = commands.clone();
        let task = tokio::spawn(async move {
            while let Some(Ok(Message::Text(text))) = peer.next().await {
                let command: Value = serde_json::from_str(&text).unwrap();
                seen.lock().unwrap().push(command.clone());
                let result = reply(&command);
                let reply = json!({"id":command["id"],"result":result}).to_string();
                if peer.send(Message::Text(reply.into())).await.is_err() {
                    break;
                }
            }
        });
        (cdp, commands, task)
    }
    fn evaluated(commands: &Commands, marker: &str) -> Vec<Value> {
        commands
            .lock()
            .unwrap()
            .iter()
            .filter(|command| {
                command["method"] == "Runtime.evaluate"
                    && command["params"]["expression"]
                        .as_str()
                        .is_some_and(|expression| expression.contains(marker))
            })
            .cloned()
            .collect()
    }
    fn methods(commands: &Commands, method: &str) -> Vec<Value> {
        commands
            .lock()
            .unwrap()
            .iter()
            .filter(|command| command["method"] == method)
            .map(|command| command["params"].clone())
            .collect()
    }
    /// The JSON arguments `evaluate_with_arguments` appended to an expression.
    fn arguments(expression: &str) -> Value {
        let (_, suffix) = expression.rsplit_once(")(...").unwrap();
        serde_json::from_str(suffix.strip_suffix(')').unwrap()).unwrap()
    }
    fn row(name: &str, index: usize) -> Value {
        json!({"name":name,"index":index,"status":"unknown"})
    }

    /// A Project Sources page in memory: the Sources tab, the Add dialog, its file input and
    /// the list. `fault` names the one step that misbehaves.
    fn sources_page(
        fault: &'static str,
        listed: Vec<String>,
    ) -> impl FnMut(&Value) -> Value + Send + 'static {
        let mut listed = listed;
        let mut selected = Vec::<String>::new();
        let mut batches = 0;
        let mut confirmed = false;
        let mut reads_after_confirm = 0;
        let mut ready_reads = 0;
        let mut prepare_reads = 0;
        move |command| {
            let params = &command["params"];
            let value = match command["method"].as_str().unwrap() {
                "Input.dispatchMouseEvent" | "Runtime.releaseObject" => return json!({}),
                "DOM.getDocument" => return json!({"root":{"nodeId":1}}),
                "DOM.querySelector" => {
                    assert_eq!(
                        params["selector"],
                        "input[data-oracle-project-sources-input=\"1\"]"
                    );
                    return json!({"nodeId":if fault=="node" {0} else {7}});
                }
                "DOM.resolveNode" => {
                    assert_eq!(params["nodeId"], 7);
                    return if fault == "object" {
                        json!({"object":{}})
                    } else {
                        json!({"object":{"objectId":"PUBLIC-input-object"}})
                    };
                }
                "Runtime.callFunctionOn" => {
                    assert_eq!(params["objectId"], "PUBLIC-input-object");
                    json!(fault != "foreign")
                }
                "DOM.setFileInputFiles" => {
                    assert_eq!(params["nodeId"], 7);
                    batches += 1;
                    selected = params["files"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|path| {
                            Path::new(path.as_str().unwrap())
                                .file_name()
                                .unwrap()
                                .to_string_lossy()
                                .into_owned()
                        })
                        .collect();
                    // The first batch lists itself; a later one needs the Upload button.
                    if batches == 1 {
                        listed.extend(selected.iter().cloned());
                    }
                    return json!({});
                }
                "Runtime.evaluate" => {
                    let expression = params["expression"].as_str().unwrap();
                    if expression.contains("alcuin-dom:submission") {
                        json!(true)
                    } else if expression.contains(ADD) {
                        assert_eq!(arguments(expression)[0]["click"], false);
                        if fault == "add" {
                            json!({"ok":false})
                        } else {
                            json!({"ok":true,"x":12.5,"y":30.0})
                        }
                    } else if expression.contains(INPUT) {
                        prepare_reads += 1;
                        json!({"ok":fault != "input" && prepare_reads % 2 == 0})
                    } else if expression.contains(CONFIRM) {
                        assert_eq!(arguments(expression)[0]["names"], json!(selected));
                        confirmed = true;
                        json!({"ok":true})
                    } else if expression.contains(LIST) {
                        if confirmed && fault != "never" {
                            reads_after_confirm += 1;
                            if reads_after_confirm == 2 {
                                listed.append(&mut selected);
                            }
                        }
                        let rows = listed
                            .iter()
                            .enumerate()
                            .map(|(index, name)| row(name, index))
                            .collect::<Vec<_>>();
                        json!({"ok":true,"sources":rows})
                    } else if expression.contains(READY) {
                        ready_reads += 1;
                        json!({"ready":fault != "unready" && ready_reads > 1})
                    } else if expression.contains(TAB) {
                        if fault == "unready" {
                            json!({"ok":false})
                        } else {
                            json!({"alreadyOpen":true})
                        }
                    } else if expression.contains("JSON.stringify(cfg.names)") {
                        assert_eq!(arguments(expression)[0]["names"], json!(selected));
                        json!(fault != "selection")
                    } else if expression.contains("delete window.__oracleProjectSourcesUpload") {
                        json!(true)
                    } else {
                        panic!(
                            "Unaccounted Sources expression: {}",
                            expression.chars().take(120).collect::<String>()
                        );
                    }
                }
                other => panic!("Unaccounted Sources page method {other}"),
            };
            json!({"result":{"value":value}})
        }
    }
    async fn on_sources_page<T>(
        fault: &'static str,
        listed: Vec<String>,
        command: Duration,
        work: impl AsyncFnOnce(&mut Surface<'_>, &CancellationToken) -> Result<T>,
    ) -> (Result<T>, Commands) {
        let (page, commands, peer) = fake_devtools(sources_page(fault, listed)).await;
        let cancel = CancellationToken::new();
        let mut reservation = RendererReservation::acquire(
            page.clone(),
            "PUBLIC-sources-claim".into(),
            command,
            &cancel,
        )
        .await
        .unwrap();
        let mut surface = Surface {
            reservation: &mut reservation,
            path: "/g/g-p-public/project".into(),
            command,
        };
        let result = work(&mut surface, &cancel).await;
        page.disconnect_and_wait(Duration::from_secs(1))
            .await
            .unwrap();
        peer.await.unwrap();
        (result, commands)
    }

    #[tokio::test(start_paused = true)]
    async fn the_sources_tab_is_ready_once_open_and_refuses_a_tab_it_cannot_find() {
        let (ready, commands) = on_sources_page(
            "",
            vec![],
            Duration::from_secs(5),
            async |surface, cancel| surface.ready(cancel).await,
        )
        .await;
        ready.unwrap();
        assert_eq!(evaluated(&commands, READY).len(), 2);
        assert_eq!(evaluated(&commands, TAB).len(), 1);
        // An already open tab is never clicked again.
        assert!(methods(&commands, "Input.dispatchMouseEvent").is_empty());

        let (ready, commands) = on_sources_page(
            "unready",
            vec![],
            Duration::from_secs(1),
            async |surface, cancel| surface.ready(cancel).await,
        )
        .await;
        assert_eq!(
            ready.unwrap_err().to_string(),
            "Project Sources tab did not become ready before timeout."
        );
        assert!(evaluated(&commands, TAB).len() >= 2);
        assert!(methods(&commands, "Input.dispatchMouseEvent").is_empty());
    }

    fn source_files(count: usize) -> (tempfile::TempDir, Vec<UploadFile>) {
        let home = tempfile::tempdir().unwrap();
        let files = (1..=count)
            .map(|index| {
                let path = home.path().join(format!("PUBLIC-source-{index:02}.txt"));
                std::fs::write(&path, format!("PUBLIC source {index}\n")).unwrap();
                UploadFile::open(&path).unwrap()
            })
            .collect();
        (home, files)
    }

    #[tokio::test(start_paused = true)]
    async fn uploads_go_in_batches_of_ten_and_confirm_only_a_batch_not_yet_listed() {
        let (_home, mut files) = source_files(11);
        let before = vec![row("PUBLIC-existing.txt", 0)];
        let (uploaded, commands) = on_sources_page(
            "",
            vec!["PUBLIC-existing.txt".into()],
            Duration::from_secs(5),
            async |surface, cancel| {
                surface
                    .upload(&mut files, &before, Duration::from_secs(1), cancel)
                    .await
            },
        )
        .await;
        let uploaded = uploaded.unwrap();
        let mut expected = vec!["PUBLIC-existing.txt".to_owned()];
        expected.extend((1..=11).map(|index| format!("PUBLIC-source-{index:02}.txt")));
        assert_eq!(
            uploaded
                .iter()
                .map(|row| row["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(added_sources(&before, &uploaded).len(), 11, "{uploaded:?}");
        let selections = methods(&commands, "DOM.setFileInputFiles");
        assert_eq!(
            selections
                .iter()
                .map(|selection| selection["files"].as_array().unwrap().len())
                .collect::<Vec<_>>(),
            [10, 1]
        );
        // Only the second batch, which did not list itself, pressed Upload.
        let confirms = evaluated(&commands, CONFIRM);
        assert_eq!(confirms.len(), 1);
        assert_eq!(
            arguments(confirms[0]["params"]["expression"].as_str().unwrap())[0]["names"],
            json!(["PUBLIC-source-11.txt"])
        );
        // Each Add press is a trusted move, press and release at the control's centre.
        let presses = methods(&commands, "Input.dispatchMouseEvent");
        assert_eq!(presses.len(), 6);
        assert_eq!(presses[1]["type"], "mousePressed");
        assert_eq!(
            (presses[1]["x"].as_f64(), presses[1]["y"].as_f64()),
            (Some(12.5), Some(30.0))
        );
        // Every input handle was released, and every upload marker removed.
        assert_eq!(methods(&commands, "Runtime.releaseObject").len(), 2);
        assert_eq!(
            evaluated(&commands, "delete window.__oracleProjectSourcesUpload").len(),
            2
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_upload_stops_at_the_first_step_it_cannot_confirm() {
        for (fault, message) in [
            ("add", "Unable to open the Project Sources Add dialog."),
            ("input", "Project Sources upload input did not appear."),
            ("node", "Unable to locate the Project Sources upload input."),
            ("object", "Project Sources input identity missing"),
            ("foreign", "Project Sources upload input ownership changed"),
            ("selection", "Project Sources upload selection changed"),
            (
                "never",
                "Timed out waiting for uploaded project sources: PUBLIC-source-01.txt",
            ),
        ] {
            let (_home, mut files) = source_files(1);
            let (uploaded, commands) = on_sources_page(
                fault,
                vec![],
                Duration::from_secs(1),
                async |surface, cancel| {
                    // A batch that lists itself never waits, so `never` first uploads a
                    // same-named batch that does; the second then needs a new occurrence.
                    let mut before = vec![];
                    if fault == "never" {
                        let (_placeholder, mut first) = source_files(1);
                        before = surface
                            .upload(&mut first, &[], Duration::from_secs(1), cancel)
                            .await?;
                    }
                    surface
                        .upload(&mut files, &before, Duration::from_secs(1), cancel)
                        .await
                },
            )
            .await;
            assert_eq!(uploaded.unwrap_err().to_string(), message, "{fault}");
            let selected = methods(&commands, "DOM.setFileInputFiles").len();
            assert_eq!(
                selected,
                match fault {
                    "selection" => 1,
                    "never" => 2,
                    _ => 0,
                },
                "{fault}"
            );
            if fault == "foreign" {
                assert_eq!(methods(&commands, "Runtime.releaseObject").len(), 1);
            }
        }
    }

    /// The browser a Project Sources run drives, in memory. `fault` names the one step that
    /// misbehaves.
    fn sources_browser(fault: &'static str) -> impl FnMut(&Value) -> Value + Send + 'static {
        let mut url = String::from("about:blank");
        let mut closed = false;
        let mut loaded = 0;
        move |command| {
            let params = &command["params"];
            match command["method"].as_str().unwrap() {
                "Target.createTarget" => json!({"targetId":TARGET}),
                "Target.attachToTarget" => json!({"sessionId":"PUBLIC-sources-session"}),
                "Target.getTargetInfo" => {
                    json!({"targetInfo":{"targetId":TARGET,"type":"page","url":url}})
                }
                "Target.getTargets" => {
                    let mut rows = vec![json!({"targetId":"PUBLIC-other-page","type":"page"})];
                    if !closed {
                        rows.push(json!({"targetId":TARGET,"type":"page","url":url}));
                    }
                    json!({"targetInfos":rows})
                }
                "Target.closeTarget" => {
                    assert_eq!(params["targetId"], TARGET);
                    closed = fault != "close";
                    json!({"success":true})
                }
                "Page.enable" => json!({}),
                "Network.getAllCookies" => json!({"cookies":[]}),
                "Network.setCookie" => json!({"success":fault != "cookies"}),
                "Page.navigate" => {
                    url = params["url"].as_str().unwrap().to_owned();
                    json!({"frameId":"PUBLIC-frame"})
                }
                "Runtime.evaluate" => {
                    let expression = params["expression"].as_str().unwrap();
                    let value = if expression == "document.readyState" {
                        json!("complete")
                    } else if expression.contains("alcuin-dom:project-sources") {
                        if expression.contains(READY) {
                            json!({"ready":true})
                        } else if expression.contains(LIST) && fault == "list" {
                            json!({"ok":false,"error":"PUBLIC list failure"})
                        } else if expression.contains(LIST) {
                            json!({"ok":true,"sources":[row("PUBLIC-notes.txt",0)]})
                        } else {
                            panic!("Unaccounted Sources body")
                        }
                    } else if expression.contains("alcuin-dom:submission") {
                        json!(true)
                    } else if expression.contains("alcuin-dom:health-login") {
                        json!({"ok":true,"status":200,"sessionAuthenticated":true})
                    } else if expression.contains("alcuin-dom:unused-blank") {
                        json!(url == "about:blank")
                    } else if expression.contains("searchParams.get('tab')==='sources'") {
                        // The project page finishes loading on the second look.
                        loaded += 1;
                        json!(loaded > 1)
                    } else {
                        panic!(
                            "Unaccounted run expression: {}",
                            expression.chars().take(120).collect::<String>()
                        )
                    };
                    json!({"result":{"value":value}})
                }
                other => panic!("Unaccounted run method {other}"),
            }
        }
    }
    /// One `run` against `sources_browser`, owning an already exited process when `owned`.
    async fn run_on_fake_browser(
        config: Value,
        environment: &Environment,
        bootstrap: Option<&crate::browser_auth::CookieBootstrap>,
        owned: bool,
        fault: &'static str,
    ) -> (Result<Value>, Commands, Vec<String>) {
        use std::process::{Command, Stdio};
        let home = tempfile::tempdir().unwrap();
        let bringup = home.path().join("PUBLIC-bringup.lock");
        let (browser, commands, peer) = fake_devtools(sources_browser(fault)).await;
        let child = owned.then(|| {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "PUBLIC-no-such-test"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            child.wait().unwrap();
            child
        });
        let endpoint = crate::browser_chrome::AttachmentEndpoint {
            host: "127.0.0.1".into(),
            port: 9,
            browser_ws_endpoint: "ws://127.0.0.1:9/devtools/browser/PUBLIC".into(),
        };
        let mut configured = crate::browser_chrome::ConfiguredChrome {
            profile: crate::browser_profile::ProfileRegistry::open(
                &home.path().join("PUBLIC-profile"),
            )
            .unwrap(),
            connection: ChromeConnection::fixture(endpoint, browser, child, None),
            source_profile: None,
            notes: vec![],
        };
        let cancel = CancellationToken::new();
        // `cancelled` stops a run that already holds its seeding lock before it takes a tab.
        if fault == "cancelled" {
            let lock = DirectoryLock::acquire(&bringup, Duration::from_secs(1)).unwrap();
            configured.connection = configured.connection.with_bootstrap_lock(lock);
            cancel.cancel();
        }
        let lines = std::sync::Mutex::new(Vec::<String>::new());
        let result = TEST_CHROME
            .scope(
                std::cell::RefCell::new(Some(configured)),
                run(
                    Request {
                        operation: "list",
                        project_url: PROJECT,
                        files: &[],
                        config,
                    },
                    environment,
                    home.path(),
                    &cancel,
                    bootstrap,
                    &|line| lines.lock().unwrap().push(line.to_owned()),
                ),
            )
            .await;
        peer.await.unwrap();
        if fault == "cancelled" {
            // The run released the seeding lock it was handed.
            DirectoryLock::acquire(&bringup, Duration::from_secs(1)).unwrap();
        }
        (result, commands, lines.into_inner().unwrap())
    }

    #[tokio::test]
    async fn a_listed_project_reads_its_sources_on_the_signed_in_page_and_closes_only_its_tab() {
        let (result, commands, lines) = run_on_fake_browser(
            json!({"manualLogin":true,"keepBrowser":true}),
            &Environment::new(),
            None,
            false,
            "",
        )
        .await;
        let value = result.unwrap();
        assert_eq!(value["status"], "ok");
        assert_eq!(value["operation"], "list");
        assert_eq!(value["projectUrl"], PROJECT);
        assert_eq!(value["dryRun"], false);
        assert_eq!(value["sourcesBefore"], json!([row("PUBLIC-notes.txt", 0)]));
        assert_eq!(value["sourcesAfter"], value["sourcesBefore"]);
        assert_eq!(value["added"], json!([]));
        assert_eq!(value["plannedUploads"], json!([]));
        assert_eq!(value["warnings"], json!([]));
        assert!(lines[0].starts_with("Manual login mode enabled; reusing persistent profile at "));
        assert!(lines.contains(&skipped_cookies_line(&json!({"manualLogin":true})).to_owned()));
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("Login check passed"))
        );
        assert_eq!(
            methods(&commands, "Page.navigate"),
            [
                json!({"url":"https://chatgpt.com/"}),
                json!({"url":PROJECT})
            ]
        );
        assert!(methods(&commands, "Network.setCookie").is_empty());
        assert_eq!(methods(&commands, "Target.closeTarget").len(), 1);
        // The reservation was taken and released cleanly, never quarantined.
        let actions = evaluated(&commands, "alcuin-dom:submission")
            .iter()
            .map(|command| {
                arguments(command["params"]["expression"].as_str().unwrap())[0]["action"].clone()
            })
            .collect::<Vec<_>>();
        assert_eq!(actions.first(), Some(&json!("acquire")));
        assert_eq!(
            actions[actions.len() - 3..],
            [json!("releasing"), json!("clean"), json!("release")]
        );
        assert!(!actions.contains(&json!("quarantine")));
    }

    #[cfg(any(windows, target_os = "linux"))]
    #[tokio::test]
    async fn extracted_chrome_cookies_seed_an_owned_browser_that_is_retired_then_retained() {
        let fixture = tempfile::tempdir().unwrap();
        let (source, environment) = crate::browser_cookie_loader::synthetic_source(fixture.path());
        let (result, commands, lines) = run_on_fake_browser(
            json!({"manualLogin":false,"cookieSync":true,"chromeCookiePath":source,"cookieNames":["PUBLIC-session"]}),
            &environment,
            None,
            true,
            "",
        )
        .await;
        let value = result.unwrap();
        assert_eq!(value["sourcesBefore"], json!([row("PUBLIC-notes.txt", 0)]));
        assert_eq!(lines[0], crate::browser_auth::CHROME_COOKIE_SYNC_WARNING);
        let cookies = methods(&commands, "Network.setCookie");
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0]["name"], "PUBLIC-session");
        assert_eq!(cookies[0]["domain"], ".chatgpt.com");
        // The tab closed and its absence was confirmed, but Chrome kept a foreign page, so
        // the owned process is retained with a warning rather than terminated.
        assert_eq!(methods(&commands, "Target.closeTarget").len(), 1);
        let warnings = value["warnings"].as_array().unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].as_str().unwrap().starts_with(
                "Chrome retained because final owned-process termination was unconfirmed: Chrome still contains foreign or unretired targets"
            ),
            "{warnings:?}"
        );
    }

    #[tokio::test]
    async fn a_failed_run_quarantines_its_reservation_or_retires_its_unused_tab() {
        // A list that cannot be read quarantines the page it reserved.
        let (result, commands, _) = run_on_fake_browser(
            json!({"manualLogin":true}),
            &Environment::new(),
            None,
            false,
            "list",
        )
        .await;
        assert_eq!(result.unwrap_err().to_string(), "PUBLIC list failure");
        let actions = evaluated(&commands, "alcuin-dom:submission")
            .iter()
            .map(|command| {
                arguments(command["params"]["expression"].as_str().unwrap())[0]["action"].clone()
            })
            .collect::<Vec<_>>();
        assert_eq!(actions.last(), Some(&json!("quarantine")));
        assert!(methods(&commands, "Target.closeTarget").is_empty());

        // Explicit cookies that apply to nothing stop before navigation, and the still blank
        // tab is retired.
        let input = json!({"browserInlineCookies":json!([{"name":"PUBLIC-cookie","value":format!("fixture-{}", std::process::id()),"domain":"chatgpt.com"}]).to_string()});
        let home = tempfile::tempdir().unwrap();
        let batch = crate::browser_auth::CookieBootstrap::resolve_explicit(
            &input,
            &json!({}),
            &Environment::new(),
            home.path(),
        )
        .unwrap()
        .unwrap();
        let (result, commands, _) = run_on_fake_browser(
            json!({"manualLogin":false,"cookieBootstrapRequired":batch.requirement()}),
            &Environment::new(),
            Some(&batch),
            true,
            "cookies",
        )
        .await;
        assert_eq!(
            result.unwrap_err().to_string(),
            "No inline cookies were applied; aborting before navigation."
        );
        assert_eq!(methods(&commands, "Network.setCookie").len(), 1);
        assert!(methods(&commands, "Page.navigate").is_empty());
        assert!(!evaluated(&commands, "alcuin-dom:unused-blank").is_empty());
        assert_eq!(methods(&commands, "Target.closeTarget").len(), 1);

        // A completed list whose tab never closes reports the cleanup failure.
        crate::browser_target::TEST_CLOSE_WAIT.set(Some(Duration::from_millis(50)));
        let (result, _, _) = run_on_fake_browser(
            json!({"manualLogin":true}),
            &Environment::new(),
            None,
            false,
            "close",
        )
        .await;
        let owned = run_on_fake_browser(
            json!({"manualLogin":true}),
            &Environment::new(),
            None,
            true,
            "close",
        )
        .await;
        crate::browser_target::TEST_CLOSE_WAIT.set(None);
        // An owned tab that never closes is a cleanup failure too, and Chrome is not terminated.
        // Which deadline check reports the unconfirmed close depends on scheduling.
        let owned_error = format!("{:#}", owned.0.unwrap_err());
        assert!(
            owned_error.starts_with(&format!(
                "Project Sources cleanup: target={TARGET}, release=Err("
            )) && owned_error.contains("retained=Ok(()), disconnect=Ok(())")
                && owned_error.ends_with("Project Sources completed; browser cleanup failed"),
            "{owned_error}"
        );
        assert_eq!(methods(&owned.1, "Target.closeTarget").len(), 1);
        let error = result.unwrap_err();
        assert!(
            error.to_string().starts_with(&format!(
                "Project Sources cleanup: target={TARGET}, release=Err("
            )),
            "{error:#}"
        );
        assert!(
            format!("{error:#}").ends_with("Project Sources completed; browser cleanup failed"),
            "{error:#}"
        );
    }

    #[tokio::test]
    async fn a_cancelled_seeded_run_stops_before_it_takes_a_tab() {
        let input = json!({"browserInlineCookies":json!([{"name":"PUBLIC-cookie","value":format!("fixture-{}", std::process::id()),"domain":"chatgpt.com"}]).to_string()});
        let home = tempfile::tempdir().unwrap();
        let batch = crate::browser_auth::CookieBootstrap::resolve_explicit(
            &input,
            &json!({}),
            &Environment::new(),
            home.path(),
        )
        .unwrap()
        .unwrap();
        let (result, commands, _) = run_on_fake_browser(
            json!({"manualLogin":false,"cookieBootstrapRequired":batch.requirement(),"closeOwnedTabOnCancel":false}),
            &Environment::new(),
            Some(&batch),
            true,
            "cancelled",
        )
        .await;
        assert_eq!(
            format!("{:#}", result.unwrap_err()),
            "Browser operation was cancelled"
        );
        // No tab was allocated, so none was closed and no cookie was written.
        assert!(commands.lock().unwrap().is_empty(), "{commands:?}");
    }
    #[tokio::test]
    async fn bootstrap_input_must_match_the_request_and_a_manual_profile_must_be_initialised() {
        let home = tempfile::tempdir().unwrap();
        let input = json!({"browserInlineCookies":json!([{"name":"PUBLIC-cookie","value":format!("fixture-{}", std::process::id())}]).to_string()});
        let batch = crate::browser_auth::CookieBootstrap::resolve_explicit(
            &input,
            &json!({}),
            &Environment::new(),
            home.path(),
        )
        .unwrap()
        .unwrap();
        let attempt = async |config: Value, environment: &Environment| {
            run(
                Request {
                    operation: "list",
                    project_url: PROJECT,
                    files: &[],
                    config,
                },
                environment,
                home.path(),
                &CancellationToken::new(),
                Some(&batch),
                &|_| {},
            )
            .await
            .unwrap_err()
            .to_string()
        };
        assert_eq!(
            attempt(json!({}), &Environment::new()).await,
            "Project Sources requires fresh cookie bootstrap input"
        );
        // A matching, usable batch passes its checks and reaches the browser host check.
        assert_eq!(
            attempt(
                json!({"cookieBootstrapRequired":batch.requirement(),"remoteChrome":{"host":"127.0.0.1","port":9}}),
                &Environment::new()
            )
            .await,
            "Project Sources v1 uses local browser automation only. Run it on the signed-in browser host."
        );
        // Without an injected browser, an uninitialised manual-login profile refuses before
        // any Chrome starts.
        let profile = home.path().join("PUBLIC-manual-profile");
        let error = run(
            Request {
                operation: "list",
                project_url: PROJECT,
                files: &[],
                config: json!({"manualLogin":true,"manualLoginProfileDir":profile}),
            },
            &Environment::from([
                ("HOME".into(), home.path().to_string_lossy().into_owned()),
                (
                    "USERPROFILE".into(),
                    home.path().to_string_lossy().into_owned(),
                ),
            ]),
            home.path(),
            &CancellationToken::new(),
            None,
            &|_| {},
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(
            error.starts_with(&format!(
                "ChatGPT browser manual-login profile is not initialized. Browser mode is using Alcuin's private Chrome profile at {}",
                profile.display()
            )),
            "{error}"
        );
    }
}
#[cfg(test)]
#[path = "project_sources_coverage2_tests.rs"]
mod coverage2_tests;
#[cfg(test)]
#[path = "project_sources_coverage_tests.rs"]
mod coverage_tests;
