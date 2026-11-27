//! Collect the files and images a completed answer delivers, under the same witnesses as its capture.
use super::*;

#[allow(clippy::too_many_arguments)] // Publication binds the same native and renderer witnesses as answer capture.
pub(super) async fn collect_deliveries(
    store: &SessionStore,
    id: &str,
    witness: &ExecutionWitness,
    outcome: &OutcomeWitness,
    target: &OwnedTarget,
    endpoint: &AttachmentEndpoint,
    reservation: &RendererReservation,
    observed: &Value,
    limits: &RunnerLimits,
    recovery: bool,
    cancel: &CancellationToken,
) -> Result<Vec<Value>> {
    let metadata = store.require(id)?;
    outcome.require(&metadata, witness, None, cancel)?;
    let requested = crate::browser_images::requested_destination(&metadata["options"])?.is_some();
    let followups = crate::browser_turns::prompts(&metadata["options"])?;
    let optional_images = !followups.is_empty()
        && (crate::browser_turns::index(&metadata)? < followups.len()
            || !prior_turn_images(store, &metadata)?.is_empty());
    let from_current_response = |artifact: &Value| {
        artifact["kind"] == "image"
            && artifact["origin"]["targetId"] == target.id()
            && artifact["origin"]["assistantMessageId"] == observed["lastAssistantMessageId"]
            && artifact["origin"]["assistantTurnId"] == observed["lastAssistantTurnId"]
    };
    anyhow::ensure!(
        !recovery
            || metadata["browser"]["runtime"]["downloadContextSettled"] == true
            || metadata["browser"]["runtime"]["downloadContextSettled"].is_null()
                && metadata["browser"]["runtime"]["downloadStage"].is_null(),
        "Native download ownership is unsettled; preserve the target and staging for explicit reconciliation without replaying a click"
    );
    let mut direct_failed = false;
    let saved_buttons = metadata["artifacts"].as_array().is_some_and(|artifacts| {
        artifacts
            .iter()
            .any(|artifact| from_current_response(artifact) && artifact["imageButton"] == true)
    });
    if !requested
        || !saved_buttons
            && ["images", "files"].iter().any(|field| {
                observed[*field]
                    .as_array()
                    .is_some_and(|items| !items.is_empty())
            })
    {
        let saved = collect_delivery_inventory(
            store,
            id,
            witness,
            outcome,
            target,
            endpoint,
            reservation,
            observed,
            limits,
            recovery,
            false,
            cancel,
        )
        .await?;
        if !requested || saved.iter().any(&from_current_response) {
            return Ok(saved);
        }
        direct_failed = observed["images"]
            .as_array()
            .is_some_and(|images| !images.is_empty());
    }
    let receipt: CommitReceipt =
        serde_json::from_value(metadata["browser"]["runtime"]["userCommitReceipt"].clone())?;
    let mut latest = observed.clone();
    let deadline = Instant::now()
        + limits
            .answer
            .clamp(Duration::from_secs(15), Duration::from_secs(900));
    // Reference checkBlockingUiWarning in pollGeneratedImageOrTextAssistantResponse.
    let blocking = || async move {
        if recovery {
            return Ok(());
        }
        match crate::browser_page::ui_warning(
            reservation.page(),
            "generated image artifacts",
            limits.command,
            cancel,
            &|line| page_log(store, id, line),
        )
        .await
        {
            Some(warning) => Err(anyhow::anyhow!(warning)),
            None => Ok(()),
        }
    };
    for attempt in 0..2 {
        blocking().await?;
        outcome.require(&store.require(id)?, witness, None, cancel)?;
        let buttons = reservation.page().evaluate_with_arguments(
            &image_button_expression(),
            &json!([{"owner":reservation.owner(),"receipt":receipt,"recovery":recovery,"buttonReceiptId":Uuid::new_v4(),"expectedMessageId":observed["lastAssistantMessageId"],"expectedTurnId":observed["lastAssistantTurnId"]}]),
            limits.command,cancel,
        ).await?;
        anyhow::ensure!(
            buttons.is_array(),
            "Owned image button inventory is unavailable"
        );
        let current = store.require(id)?;
        outcome.require(&current, witness, None, cancel)?;
        let mut image_buttons = vec![];
        for button in buttons.as_array().unwrap() {
            if let Some(file) = current["artifacts"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|file| matches_saved_file(file, button, target.id(), observed))
                && crate::browser_download::image_type(&crate::browser_output::saved_file_bytes(
                    file, cancel,
                )?)
                .is_none()
            {
                continue;
            }
            image_buttons.push(button.clone());
        }
        let had_buttons = !image_buttons.is_empty();
        let mut button_observation = latest.clone();
        button_observation["images"] = json!(image_buttons);
        button_observation["files"] = json!([]);
        let saved = collect_delivery_inventory(
            store,
            id,
            witness,
            outcome,
            target,
            endpoint,
            reservation,
            &button_observation,
            limits,
            recovery,
            true,
            cancel,
        )
        .await?;
        if saved.iter().any(&from_current_response) {
            return Ok(saved);
        }
        if optional_images && !direct_failed {
            anyhow::ensure!(
                !had_buttons,
                "Required browser image delivery is incomplete; preserve the conversation and captured images for recovery."
            );
            return Ok(saved);
        }
        if attempt == 1 || direct_failed {
            break;
        }
        while Instant::now() < deadline {
            pause(Duration::from_millis(1500), cancel).await?;
            blocking().await?;
            latest = reservation
                .page()
                .evaluate_with_arguments(
                    &result_expression(),
                    &json!([{"owner":reservation.owner(),"receipt":receipt,"recovery":recovery}]),
                    limits.command,
                    cancel,
                )
                .await?;
            anyhow::ensure!(
                latest["pending"] != true
                    && latest["lastUserMessageId"] == observed["lastUserMessageId"]
                    && latest["lastAssistantMessageId"] == observed["lastAssistantMessageId"]
                    && latest["lastAssistantTurnId"] == observed["lastAssistantTurnId"],
                "Required image polling changed the owned assistant response"
            );
            let current = store.require(id)?;
            outcome.require(&current, witness, None, cancel)?;
            browser::require_original_source_pair(&current, &latest)?;
            if latest["images"]
                .as_array()
                .is_some_and(|images| !images.is_empty())
                && latest["completionVisible"] == true
                && latest["stopExists"] == false
                && latest["strongThinkingActive"] == false
            {
                let saved = collect_delivery_inventory(
                    store,
                    id,
                    witness,
                    outcome,
                    target,
                    endpoint,
                    reservation,
                    &latest,
                    limits,
                    recovery,
                    false,
                    cancel,
                )
                .await?;
                if saved.iter().any(|artifact| artifact["kind"] == "image") {
                    return Ok(saved);
                }
                direct_failed = true;
                break;
            }
        }
    }
    if direct_failed {
        bail!(
            "Required browser image delivery is incomplete; preserve the conversation and captured images for recovery."
        );
    }
    let text = latest["text"]
        .as_str()
        .filter(|text| !policy::trim(text).is_empty())
        .unwrap_or("(empty response)");
    bail!("No images generated. Response text:\n{text}")
}
fn prior_turn_images(store: &SessionStore, metadata: &Value) -> Result<Vec<Value>> {
    if crate::browser_turns::prompts(&metadata["options"])?.is_empty() {
        return Ok(vec![]);
    }
    Ok(
        turn_image_deliveries(store, metadata, crate::browser_turns::index(metadata)?)?
            .into_iter()
            .flat_map(|delivery| delivery.saved_images().to_vec())
            .collect(),
    )
}
pub(super) fn turn_image_deliveries(
    store: &SessionStore,
    metadata: &Value,
    count: usize,
) -> Result<Vec<ImageDelivery>> {
    let index = crate::browser_turns::index(metadata)?;
    let turns = crate::browser_turns::saved(metadata)?;
    anyhow::ensure!(
        turns.len() >= index && turns.len() <= index + 1 && count <= turns.len(),
        "Sequential artifact progress changed"
    );
    let cwd = Path::new(
        metadata["cwd"]
            .as_str()
            .context("Sequential artifacts have no saved working directory")?,
    );
    let directory = store
        .directory(
            metadata["id"]
                .as_str()
                .context("Sequential artifacts have no session")?,
        )?
        .join("artifacts");
    let mut deliveries = vec![];
    let mut previous_count = 0;
    for (i, turn) in turns.iter().take(count).enumerate() {
        crate::browser_turns::verify_turn(store, metadata, turn, i)?;
        let origin = json!({"targetId":metadata["browser"]["runtime"]["chromeTargetId"],"assistantMessageId":turn["assistantMessageId"],"assistantTurnId":turn["assistantTurnId"]});
        let receipts = metadata["artifacts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|row| {
                row["kind"] == "image"
                    && row["origin"]["targetId"] == origin["targetId"]
                    && row["origin"]["assistantMessageId"] == origin["assistantMessageId"]
                    && row["origin"]["assistantTurnId"] == origin["assistantTurnId"]
            })
            .cloned()
            .collect::<Vec<_>>();
        let buttons = receipts
            .first()
            .is_some_and(|receipt| receipt["imageButton"] == true);
        let mut delivery = ImageDelivery::after_previous(
            &metadata["options"],
            cwd,
            &directory,
            if buttons {
                &[]
            } else {
                turn["images"]
                    .as_array()
                    .context("Prior image inventory changed")?
            },
            previous_count,
        )?;
        delivery.restore_saved(&receipts, &origin, buttons)?;
        if !delivery.generated.is_empty() || delivery.has_saved_images() {
            delivery.require_complete(&receipts)?;
        }
        previous_count = previous_count
            .checked_add(receipts.len())
            .context("Image index overflow")?;
        deliveries.push(delivery);
    }
    Ok(deliveries)
}
fn save_image_delivery(
    store: &SessionStore,
    id: &str,
    witness: &ExecutionWitness,
    outcome: &OutcomeWitness,
    delivery: &ImageDelivery,
    observed: &Value,
    cancel: &CancellationToken,
) -> Result<()> {
    let summary = delivery.summary(
        observed["text"]
            .as_str()
            .context("Owned image answer text is unavailable")?,
    );
    let mut before_delivery = None;
    let images = if summary["buttonDownloads"] == true {
        json!([])
    } else {
        json!(delivery.generated)
    };
    let after_delivery = outcome.update(store,id,witness,cancel,|current| {
        let mut changes = json!({"imageDelivery":summary});
        let accepted = &current["browser"]["runtime"]["acceptedObservation"];
        if accepted.is_object() && summary["buttonDownloads"] != true {
            browser::require_original_source_pair(current, observed)?;
            let provider_text = observed.get("providerText").unwrap_or(&observed["text"]);
            anyhow::ensure!(accepted["lastUserMessageId"]==observed["lastUserMessageId"]
                && accepted["lastAssistantMessageId"]==observed["lastAssistantMessageId"]
                && accepted["lastAssistantTurnId"]==observed["lastAssistantTurnId"]
                && accepted.get("providerText").unwrap_or(&accepted["text"])==provider_text
                && observed["completionVisible"]==true && observed["stopExists"]==false
                && observed["strongThinkingActive"]==false,
                "Accepted response changed during artifact delivery");
            let mut refreshed = merge_object(accepted, observed);
            refreshed["providerText"] = provider_text.clone();
            refreshed["images"] = json!(delivery.generated);
            refreshed["text"] = json!(browser::normalise_image_answer(provider_text.as_str().context("Delivered response has no provider text")?,delivery.generated.len()));
            changes["acceptedObservation"] = refreshed;
        }
        if !crate::browser_turns::prompts(&current["options"])?.is_empty() {
            let index = crate::browser_turns::index(current)?;
            let mut turns = crate::browser_turns::saved(current)?.to_vec();
            let row = turns.get_mut(index).context("Delivered image turn has no saved capture")?;
            crate::browser_turns::verify_turn(store,current,row,index)?;
            anyhow::ensure!(row["assistantMessageId"]==observed["lastAssistantMessageId"]&&row["assistantTurnId"]==observed["lastAssistantTurnId"],"Delivered image response changed");
            row["images"] = images.clone();
            changes["completedBrowserTurns"] = json!(turns);
            before_delivery = Some(current.clone());
        }
        Ok(Some(json!({"browser":merge_object(&current["browser"],&json!({"runtime":merge_object(&current["browser"]["runtime"],&changes)}))})))
    })?;
    if let Some(before) = before_delivery {
        witness.seal_recovered_delivery(&before, &after_delivery, &images)?;
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)] // Every publication is tied to the native and renderer witnesses.
async fn collect_delivery_inventory(
    store: &SessionStore,
    id: &str,
    witness: &ExecutionWitness,
    outcome: &OutcomeWitness,
    target: &OwnedTarget,
    endpoint: &AttachmentEndpoint,
    reservation: &RendererReservation,
    observed: &Value,
    limits: &RunnerLimits,
    recovery: bool,
    buttons: bool,
    cancel: &CancellationToken,
) -> Result<Vec<Value>> {
    let committed = request(store, id)?;
    let metadata = store.require(id)?;
    outcome.require(&metadata, witness, None, cancel)?;
    anyhow::ensure!(
        sha256(&serde_json::to_vec(&metadata["options"])?) == committed.options_sha256,
        "Browser committed options changed before artifact publication"
    );
    let directory = store.directory(id)?.join("artifacts");
    let cwd = Path::new(
        metadata["cwd"]
            .as_str()
            .context("Image delivery has no saved working directory")?,
    );
    let prior_images = prior_turn_images(store, &metadata)?;
    let mut image_delivery = ImageDelivery::after_previous(
        &metadata["options"],
        cwd,
        &directory,
        if buttons {
            &[]
        } else {
            observed["images"]
                .as_array()
                .context("Owned image inventory is unavailable")?
                .as_slice()
        },
        prior_images.len(),
    )?;
    let previous = metadata["artifacts"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let current_previous = previous
        .iter()
        .filter(|row| !prior_images.contains(row))
        .cloned()
        .collect::<Vec<_>>();
    image_delivery.restore_saved(&current_previous,&json!({"targetId":target.id(),"assistantMessageId":observed["lastAssistantMessageId"],"assistantTurnId":observed["lastAssistantTurnId"]}),buttons)?;
    let mut candidates = vec![];
    let mut identities = std::collections::BTreeSet::new();
    for (kind, field) in [("image", "images"), ("file", "files")] {
        let inventory = if kind == "image" && !buttons {
            &image_delivery.generated
        } else {
            observed[field]
                .as_array()
                .context("Owned artifact inventory is unavailable")?
        };
        for candidate in inventory {
            if kind == "file"
                && let Some(saved) = previous
                    .iter()
                    .find(|saved| matches_saved_file(saved, candidate, target.id(), observed))
            {
                crate::browser_output::saved_file_bytes(saved, cancel)?;
                continue;
            }
            if kind == "image"
                && image_delivery.saved_images().iter().any(|saved| {
                    if buttons {
                        saved["candidateId"] == candidate["candidateId"]
                            && saved["filename"] == candidate["filename"]
                    } else {
                        saved["fileId"] == candidate["fileId"]
                    }
                })
            {
                continue;
            }
            let raw = candidate["url"]
                .as_str()
                .context("Owned artifact has no provider identity")?;
            let key = if raw == "browser-download" {
                format!(
                    "control:{}",
                    candidate["candidateId"]
                        .as_str()
                        .context("Download control has no stable candidate identity")?
                )
            } else {
                browser_download::download_url(raw)?.to_string()
            };
            if identities.insert(key) {
                candidates.push((kind, candidate.clone()));
            }
        }
    }
    // A remote browser would write into its own host's filesystem. Requested images still fail
    // through require_complete; file candidates stay unsaved, as the reference reports.
    if remote_host(endpoint) && !candidates.is_empty() {
        let files = candidates
            .iter()
            .filter(|(kind, _)| *kind == "file")
            .count();
        if files > 0 {
            automation_log(
                store,
                id,
                &format!(
                    "[browser] WARNING: {files} downloadable candidate(s) existed, but no local browser-host artifact was saved; bridge artifact-ready will not be emitted until ChatGPT file capture succeeds."
                ),
            )?;
        }
        candidates.clear();
    }
    if candidates.is_empty() {
        if !image_delivery.generated.is_empty() || image_delivery.has_saved_images() {
            image_delivery.require_complete(&previous)?;
        }
        if image_delivery.has_saved_images() {
            save_image_delivery(
                store,
                id,
                witness,
                outcome,
                &image_delivery,
                observed,
                cancel,
            )?;
        }
        return Ok(previous);
    }
    outcome.require(&store.require(id)?, witness, None, cancel)?;
    let home = store
        .home()
        .parent()
        .context("Native download ownership home is unavailable")?;
    outcome.runtime(
        store,
        id,
        witness,
        cancel,
        json!({"downloadContextSettled":false}),
    )?;
    let mut owner =
        DownloadOwner::begin(target, endpoint, home, &directory, limits.command, cancel).await?;
    outcome.runtime(
        store,
        id,
        witness,
        cancel,
        json!({"downloadStage":owner.stage_path()}),
    )?;
    let receipt: CommitReceipt = serde_json::from_value(
        store.require(id)?["browser"]["runtime"]["userCommitReceipt"].clone(),
    )?;
    let function = download_click_expression();
    let saved=async {
        let mut saved=previous;
        let mut settled_image_failure=false;
        for (kind,candidate) in candidates {
            outcome.require(&store.require(id)?,witness,None,cancel)?;
            let raw=candidate["url"].as_str().unwrap();
            let url=if raw=="browser-download" {Value::Null}else{json!(browser_download::download_url(raw)?.as_str())};
            let cfg=json!({"owner":reservation.owner(),"receipt":receipt,"recovery":recovery,"candidate":candidate,"kind":kind,"imageButton":buttons,"downloadUrl":url,"expectedMessageId":observed["lastAssistantMessageId"],"expectedTurnId":observed["lastAssistantTurnId"]});
            let (bytes,row)=owner.capture(reservation.page(),&function,cfg,limits.command,cancel).await?;
            outcome.require(&store.require(id)?,witness,None,cancel)?;
            let current=reservation.page().evaluate_with_arguments(&result_expression(),&json!([{"owner":reservation.owner(),"receipt":receipt,"recovery":recovery}]),limits.command,cancel).await?;
            anyhow::ensure!(current["lastAssistantMessageId"]==observed["lastAssistantMessageId"]&&current["lastAssistantTurnId"]==observed["lastAssistantTurnId"]&&current["completionVisible"]==true&&current["stopExists"]==false&&current["strongThinkingActive"]==false,"Result identity or completion changed before artifact publication; staging retained");
            let current_meta=store.require(id)?;
            outcome.require(&current_meta,witness,None,cancel)?;
            browser::require_original_source_pair(&current_meta,&current)?;
            if kind=="image" && browser_download::image_type(&bytes).is_none() {
                owner.discard_non_image(row["guid"].as_str().context("Owned download GUID missing")?,&bytes)?;
                if !buttons {settled_image_failure=true;saved.push(json!({"kind":"warning","code":"image-download-failed","message":"Generated image download returned no verified image signature","fallbackAllowed":true}));outcome.update(store,id,witness,cancel,|current|Ok(Some(json!({"artifacts":sessions::merge_artifacts(&current["artifacts"],&json!(saved),&metadata["artifacts"])}))))?;break;}
                continue;
            }
            let suggested=row["suggestedFilename"].as_str().unwrap_or("download.bin");
            let name=candidate["filename"].as_str().filter(|s|!s.is_empty()).unwrap_or(suggested);
            let origin=json!({"mode":"local","targetId":target.id(),"frameId":row["frameId"],"downloadGuid":row["guid"],"assistantMessageId":observed["lastAssistantMessageId"],"assistantTurnId":observed["lastAssistantTurnId"]});
            let mut image_candidate=candidate.clone();image_candidate["origin"]=origin.clone();
            if buttons {image_candidate["alt"]=candidate["label"].clone();}
            let mut publication=None;
            let mut automatic_save_error=None;
            outcome.update(store,id,witness,cancel,|current| {
                let result=if kind=="image" {image_delivery.publish(&bytes,&image_candidate,cancel)}else{browser_download::publish_download(&bytes,&directory,name,false,cancel)};
                let manifest=match result {
                    Ok(manifest)=>manifest,
                    Err(error) if kind=="image"&&!image_delivery.requested&&!cancel.is_cancelled()=>{automatic_save_error=Some(error);return Ok(None)},
                    Err(error)=>return Err(error),
                };
                let mut manifest=manifest;
                if kind=="file" {manifest["sourceUrl"]=candidate["url"].clone();manifest["filename"]=json!(name);manifest["candidateId"]=candidate["candidateId"].clone();}
                if buttons {image_delivery.record_button_image(&image_candidate);}
                manifest["origin"]=origin;
                owner.publication_confirmed(row["guid"].as_str().unwrap(),&manifest)?;
                publication=Some(manifest.clone());
                Ok(Some(json!({"artifacts":sessions::merge_artifacts(&current["artifacts"],&json!([manifest]),&Value::Null)})))
            })?;
            if let Some(error)=automatic_save_error {
                saved.push(json!({"kind":"warning","code":"image-download-failed","message":error.to_string(),"fallbackAllowed":false}));
                outcome.update(store,id,witness,cancel,|current|Ok(Some(json!({"artifacts":sessions::merge_artifacts(&current["artifacts"],&json!(saved),&metadata["artifacts"])}))))?;
                break;
            }
            let manifest=publication.context("Artifact publication produced no receipt")?;
            saved.push(manifest.clone());
        }
        if (!image_delivery.generated.is_empty() || image_delivery.has_saved_images()) && !(image_delivery.requested && settled_image_failure && !image_delivery.has_saved_images()) {
            image_delivery.require_complete(&saved)?;
            save_image_delivery(store,id,witness,outcome,&image_delivery,observed,cancel)?;
        }
        Ok::<_,anyhow::Error>(saved)
    }.await;
    let finished = owner.finish(Duration::from_secs(1)).await;
    if finished.is_ok() {
        outcome.download_settled(store, id, witness)?;
    }
    match (saved, finished) {
        (Ok(mut saved), Ok(warning)) => {
            if let Some(warning) = warning {
                saved.push(json!({"kind":"warning","code":"artifact-staging-cleanup-failed","message":warning}));
                outcome.update(store,id,witness,cancel,|current|Ok(Some(json!({"artifacts":sessions::merge_artifacts(&current["artifacts"],&json!(saved),&metadata["artifacts"])}))))?;
            }
            Ok(saved)
        }
        (Err(error), _) => Err(error),
        (_, Err(error)) => Err(error),
    }
}
fn image_button_expression() -> String {
    format!(
        "(cfg) => {{ /* alcuin-dom:image-buttons */ const readOwnedResult={}; const observed=readOwnedResult(cfg); if(observed.pending || observed.lastAssistantMessageId!==cfg.expectedMessageId || observed.lastAssistantTurnId!==cfg.expectedTurnId || !observed.completionVisible || observed.stopExists || observed.strongThinkingActive)throw Error('Owned image button response changed'); {} {} {} const turns=readTurns();const root=lastAssistant(turns[observed.lastAssistantTurnIndex]);if(!root)throw Error('Owned image assistant root is unavailable');const claim=readOracleTargetClaim();if(!matchesOracleSubmission(claim,cfg.owner))throw Error('Image button renderer ownership changed');const controls=Array.from(root.querySelectorAll('a[href],a[download],button,[role=\"button\"],[data-testid],[aria-label],[title]'));const candidates=serializeImageButtons(root,observed.lastAssistantTurnIndex);window.__alcuinImageButtonReceipts={{owner:JSON.stringify(cfg.owner),id:cfg.buttonReceiptId,nodes:new Map(candidates.map(candidate=>[candidate.candidateId,controls[Number(candidate.candidateId.split(':').at(-1))]])),spent:new Set()}};return candidates.map(candidate=>({{...candidate,buttonReceiptId:cfg.buttonReceiptId}})); }}",
        result_expression(),
        include_str!("../../browser-scripts/ownership-prelude.js"),
        include_str!("../../browser-scripts/result-prelude.js"),
        include_str!("../../browser-scripts/artifact-prelude.js")
    )
}
fn download_click_expression() -> String {
    format!(
        "(cfg) => {{ /* alcuin-dom:download-click */ const readOwnedResult = {}; const observed=readOwnedResult(cfg); if(observed.pending || observed.lastAssistantMessageId!==cfg.expectedMessageId || observed.lastAssistantTurnId!==cfg.expectedTurnId || !observed.completionVisible || observed.stopExists || observed.strongThinkingActive)throw Error('Owned download response changed'); {} {} {} const turns=readTurns(); const root=lastAssistant(turns[observed.lastAssistantTurnIndex]); if(!root)throw Error('Owned assistant root is unavailable'); const claim=readOracleTargetClaim();if(!matchesOracleSubmission(claim,cfg.owner))throw Error('Download renderer ownership changed'); const candidates=cfg.imageButton?serializeImageButtons(root,observed.lastAssistantTurnIndex):cfg.kind==='image'?serializeImages(root):serializeFiles(root,observed.lastAssistantTurnIndex); const matches=candidates.filter(item=>item.url===cfg.candidate.url && (cfg.kind==='image'&&!cfg.imageButton||item.candidateId===cfg.candidate.candidateId&&item.filename===cfg.candidate.filename)); if(matches.length<1)throw Error('Download candidate was removed or replaced'); const id=String(cfg.candidate.candidateId||'').split(':');const controls=Array.from(root.querySelectorAll('a[href],a[download],button,[role=\"button\"],[data-testid],[aria-label],[title]'));const index=Number(id.at(-1));const control=Number.isSafeInteger(index)?controls[index]:null;if(cfg.imageButton){{const seal=window.__alcuinImageButtonReceipts;if(!seal||seal.owner!==JSON.stringify(cfg.owner)||seal.id!==cfg.candidate.buttonReceiptId||seal.nodes.get(cfg.candidate.candidateId)!==control||seal.spent.has(cfg.candidate.candidateId))throw Error('Image download control ownership changed');seal.spent.add(cfg.candidate.candidateId);}} if(cfg.downloadUrl){{ const anchor=document.createElement('a');anchor.href=cfg.downloadUrl;anchor.download=cfg.candidate.filename||'download';anchor.rel='noopener';anchor.style.display='none';document.body.appendChild(anchor);anchor.click();setTimeout(()=>anchor.remove(),0);return {{clicked:1}}; }} if(!control || control.disabled || control.getAttribute('aria-disabled')==='true')throw Error('Download control is unavailable'); const again=serializeCandidate(control,cfg.candidate.candidateId,cfg.imageButton);if(!again || again.filename!==cfg.candidate.filename || again.url!==cfg.candidate.url)throw Error('Download control changed');control.click();return {{clicked:1}}; }}",
        result_expression(),
        include_str!("../../browser-scripts/ownership-prelude.js"),
        include_str!("../../browser-scripts/result-prelude.js"),
        include_str!("../../browser-scripts/artifact-prelude.js")
    )
}
#[cfg(test)]
#[path = "deliveries_coverage_tests.rs"]
mod coverage_tests;
