//! Evidence for one text turn in an owned Gemini Temporary Chat.
use crate::{browser_commit::SubmissionOwner, browser_runner::BrowserRequest};
use anyhow::{Result, ensure};
use serde_json::{Value, json};

pub(crate) const URL: &str = "https://gemini.google.com/app";
pub(crate) const DOM: &str = include_str!("../browser-scripts/gemini.js");
pub(crate) const MODEL: &str = "Gemini 4 Neon";
pub(crate) const THINKING: &str = "High";

pub(crate) fn model() -> Value {
    json!({"verified":true,"source":"gemini-model-picker","resolvedLabel":MODEL})
}

pub(crate) fn thinking() -> Value {
    json!({"verified":true,"source":"gemini-thinking-picker","resolvedLabel":THINKING})
}

pub(crate) fn receipt(owner: &SubmissionOwner, dispatch: &Value) -> Value {
    json!({"provider":"gemini","temporaryChat":true,"model":MODEL,"thinking":THINKING,
        "owner":owner,"id":dispatch["providerDispatchId"],"promptSha256":dispatch["promptSha256"]})
}

pub(crate) fn accept_saved_answer(
    request: &BrowserRequest,
    session: &Value,
    observed: &Value,
) -> Result<()> {
    let runtime = &session["browser"]["runtime"];
    let dispatch = &session["dispatch"];
    let owner: SubmissionOwner = serde_json::from_value(runtime["submissionOwner"].clone())?;
    ensure!(
        runtime["provider"] == "gemini"
            && runtime["modelSelection"] == model()
            && runtime["thinkingSelection"] == thinking()
            && runtime["effectiveModel"] == "gemini-4-neon"
            && runtime["submittedPromptHash"] == request.prompt_sha256
            && dispatch["promptSha256"] == request.prompt_sha256
            && dispatch["reservationId"] == request.reservation_id
            && dispatch["submissionOwner"] == json!(owner)
            && dispatch["providerDispatchId"]
                .as_str()
                .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
            && runtime["geminiCommitReceipt"] == receipt(&owner, dispatch)
            && observed["kind"] == "gemini-temporary-answer"
            && observed["id"] == dispatch["providerDispatchId"]
            && observed["temporaryChat"] == true
            && observed["model"] == MODEL
            && observed["thinking"] == THINKING
            && observed["text"]
                .as_str()
                .is_some_and(|text| !text.trim().is_empty()),
        "Gemini answer lacks evidence of its original Temporary Chat, selection or dispatch"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "00000000-0000-4000-8000-000000000007";

    fn fixture() -> (BrowserRequest, Value, Value) {
        let request: BrowserRequest = serde_json::from_value(json!({"version":1,
            "sessionId":"public-gemini","reservationId":"PUBLIC-reservation","prompt":"PUBLIC prompt",
            "promptSha256":"a".repeat(64),"optionsSha256":"b".repeat(64),"uploadPaths":[]}))
        .unwrap();
        let owner = SubmissionOwner {
            claim_id: "PUBLIC-claim".into(),
            reservation_id: "PUBLIC-reservation".into(),
        };
        let dispatch = json!({"state":"acknowledged","reservationId":"PUBLIC-reservation",
            "submissionOwner":owner,"promptSha256":"a".repeat(64),"providerDispatchId":ID});
        let session = json!({"dispatch":dispatch,"browser":{"runtime":{"provider":"gemini",
            "modelSelection":model(),"thinkingSelection":thinking(),"effectiveModel":"gemini-4-neon",
            "submittedPromptHash":"a".repeat(64),"submissionOwner":owner,
            "geminiCommitReceipt":receipt(&owner, &dispatch)}}});
        let observed = json!({"kind":"gemini-temporary-answer","id":ID,"temporaryChat":true,
            "model":MODEL,"thinking":THINKING,"text":"PUBLIC answer"});
        (request, session, observed)
    }

    #[test]
    fn a_gemini_answer_needs_temporary_chat_neon_high_and_its_own_dispatch() {
        let (request, session, observed) = fixture();
        accept_saved_answer(&request, &session, &observed).unwrap();
        assert_eq!(
            session["browser"]["runtime"]["geminiCommitReceipt"]["temporaryChat"],
            true
        );
        type Edit = fn(&mut Value, &mut Value);
        let edits: [(&str, Edit); 10] = [
            ("not temporary", |_, o| o["temporaryChat"] = json!(false)),
            ("other model", |_, o| o["model"] = json!("Gemini 4 Flash")),
            ("low thinking", |_, o| o["thinking"] = json!("Low")),
            ("other turn", |_, o| o["id"] = json!("PUBLIC-other")),
            ("empty answer", |_, o| o["text"] = json!("  ")),
            ("other kind", |_, o| {
                o["kind"] = json!("grok-private-answer")
            }),
            ("other provider", |s, _| {
                s["browser"]["runtime"]["provider"] = json!("grok")
            }),
            ("other prompt", |s, _| {
                s["dispatch"]["promptSha256"] = json!("c".repeat(64))
            }),
            ("bad dispatch id", |s, _| {
                s["dispatch"]["providerDispatchId"] = json!("PUBLIC")
            }),
            ("receipt changed", |s, _| {
                s["browser"]["runtime"]["geminiCommitReceipt"]["temporaryChat"] = json!(false)
            }),
        ];
        for (case, edit) in edits {
            let (request, mut session, mut observed) = fixture();
            edit(&mut session, &mut observed);
            assert!(
                accept_saved_answer(&request, &session, &observed).is_err(),
                "{case} was accepted"
            );
        }
    }
}

#[cfg(test)]
pub(crate) async fn native_case(page: &crate::cdp::Cdp) -> Result<()> {
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    let timeout = Duration::from_secs(3);
    let cancel = CancellationToken::new();
    let prompt = "PUBLIC Zihuatanejo fixture\nSecond line";
    let dispatch = "public-gemini-dispatch";
    let action = |action| {
        let cancel = &cancel;
        async move {
            page.evaluate_with_arguments(
                DOM,
                &json!([{"action":action,"id":dispatch,"prompt":prompt,
                "owner":{"claimId":"fixture-claim","reservationId":"fixture-reservation"}}]),
                timeout,
                cancel,
            )
            .await
        }
    };
    page.evaluate(r#"
        document.body.innerHTML = '<header><button aria-label="Temporary chat">Temporary chat</button></header>\
          <button aria-label="Open mode picker, currently Auto">Modes</button>\
          <div id="menu" hidden></div><rich-textarea><div class="ql-editor" contenteditable="true" role="textbox"><p><br></p></div></rich-textarea>\
          <button aria-label="Send message">Send</button>';
        const temporary = document.querySelector('header button');
        const picker = document.querySelector('button[aria-label^="Open mode picker"]');
        const menu = document.querySelector('#menu');
        temporary.onclick = () => temporary.setAttribute('aria-pressed','true');
        picker.onclick = () => {
          menu.hidden = !menu.hidden;
          menu.innerHTML = '<gem-menu-item data-mode-id="neon">Deeper analysis 4 Neon</gem-menu-item>\
            <gem-menu-item>Thinking level</gem-menu-item>';
          menu.children[0].onclick = () => {
            picker.setAttribute('aria-label','Open mode picker, currently Neon Low');
            setTimeout(() => { menu.hidden = true; }, 100);
          };
          menu.children[1].onclick = () => {
            const level = document.createElement('gem-menu-item');
            level.innerText = 'High';
            level.onclick = () => { picker.setAttribute('aria-label','Open mode picker, currently Neon High'); menu.hidden = true; };
            menu.append(level);
          };
        };
        window.fixtureClaim = (state, retiring = false) => {
          sessionStorage.setItem('oracle:target-claim',
            JSON.stringify({id:'fixture-claim',retiring,submission:{id:'fixture-reservation',state:'active',dispatch:state}}));
          return true;
        };
        fixtureClaim('not-started');
        window.fixtureSends = 0;
        document.querySelector('button[aria-label="Send message"]').onclick = () => {
          window.fixtureSends++;
          const user = document.createElement('user-query');
          user.innerHTML = '<div class="query-text"><span class="cdk-visually-hidden">Your prompt</span></div>';
          for (const block of document.querySelector('.ql-editor').children) {
            const line = document.createElement('p');
            line.className = 'query-text-line';
            line.textContent = block.textContent;
            user.firstChild.append(line);
          }
          document.body.append(user);
          const answer = document.createElement('model-response');
          answer.innerHTML = '<message-content><div class="markdown">PUBLIC answer</div></message-content><button aria-label="Copy response">Copy</button>';
          document.body.append(answer);
        };
        true
    "#, timeout, &cancel).await?;
    assert!(action("type").await.is_err());
    assert_eq!(action("entry").await?, "clicked");
    assert_eq!(action("privacy").await?, true);
    assert_eq!(action("type").await?, "claim");
    assert_eq!(action("open-picker").await?, true);
    assert_eq!(action("show-neon").await?, false);
    let deadline = tokio::time::Instant::now() + timeout;
    while action("show-neon").await? == false {
        ensure!(
            tokio::time::Instant::now() < deadline,
            "Fixture picker did not close"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    for step in ["open-picker", "show-levels", "select-high", "seal"] {
        assert_eq!(action(step).await?, true, "{step}");
    }
    assert_eq!(action("type").await?, "entered");
    assert_eq!(action("verify-draft").await?, true);
    assert_eq!(action("send").await?, false);
    page.evaluate("fixtureClaim('started', true)", timeout, &cancel)
        .await?;
    assert_eq!(action("send").await?, false);
    page.evaluate("fixtureClaim('started')", timeout, &cancel)
        .await?;
    page.evaluate(
        r#"(document.querySelector('header button').setAttribute('aria-pressed','false'), true)"#,
        timeout,
        &cancel,
    )
    .await?;
    assert!(action("send").await.is_err());
    page.evaluate(
        r#"(document.querySelector('header button').setAttribute('aria-pressed','true'), true)"#,
        timeout,
        &cancel,
    )
    .await?;
    assert_eq!(action("send").await?, true);
    assert_eq!(action("send").await?, false);
    assert_eq!(action("observe").await?["text"], "PUBLIC answer");
    page.evaluate(r#"(document.querySelector('button[aria-label^="Open mode picker"]').setAttribute('aria-label','Open mode picker, currently Neon Low'), true)"#, timeout, &cancel).await?;
    assert!(action("observe").await.is_err());
    assert_eq!(
        page.evaluate("window.fixtureSends", timeout, &cancel)
            .await?,
        1
    );
    Ok(())
}
