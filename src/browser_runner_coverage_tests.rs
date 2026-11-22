//! Coverage tests for browser_runner helpers: navigation, image summaries and retirement reads.
use super::*;
use crate::cdp::fake::{Reply, peer};
use std::sync::Mutex;

fn quick_limits(command_ms: u64) -> RunnerLimits {
    RunnerLimits {
        command: Duration::from_millis(command_ms),
        poll: Duration::from_millis(10),
        ..RunnerLimits::default()
    }
}

/// A page whose first `failed_loads` navigations report a load error and whose first
/// `wrong_reads` location reads return a blank page before the expected URL.
async fn navigation_peer(
    failed_loads: usize,
    wrong_reads: usize,
    url: String,
) -> (Cdp, Arc<Mutex<Vec<String>>>) {
    let methods = Arc::new(Mutex::new(Vec::new()));
    let seen = methods.clone();
    let (mut loads, mut reads) = (0usize, 0usize);
    let (cdp, _task) = peer(Duration::from_secs(1), move |request| {
        seen.lock().unwrap().push(request.method.clone());
        match request.method.as_str() {
            "Page.navigate" => {
                loads += 1;
                if loads <= failed_loads {
                    Reply::Result(json!({"frameId":"PUBLIC-frame","errorText":"net::ERR_PUBLIC"}))
                } else {
                    Reply::Result(json!({"frameId":"PUBLIC-frame"}))
                }
            }
            "Runtime.evaluate" => {
                reads += 1;
                if reads <= wrong_reads {
                    Reply::value(json!("about:blank"))
                } else {
                    Reply::value(json!(url))
                }
            }
            _ => Reply::Error("PUBLIC-unexpected".into()),
        }
    })
    .await;
    (cdp, methods)
}

#[test]
fn navigation_destination_defaults_and_refusals() {
    let (url, navigation) = navigation_destination(&json!({})).unwrap();
    assert!(url.starts_with("https://chatgpt.com"), "{url}");
    assert_eq!(navigation, browser_dom::navigation(&url).unwrap());

    let resumed = navigation_destination(
        &json!({"resumeConversationUrl":"https://chatgpt.com/c/PUBLIC-conversation"}),
    )
    .unwrap_err();
    assert!(
        resumed
            .to_string()
            .contains("Temporary Chat cannot be reopened"),
        "{resumed:#}"
    );

    assert!(navigation_destination(&json!({"chatgptUrl":"not a url"})).is_err());
}

#[tokio::test]
async fn navigate_exact_retries_load_errors_and_waits_for_exact_navigation() {
    let cancel = CancellationToken::new();
    let (url, expected) = navigation_destination(&json!({})).unwrap();

    // One failed load is retried; a blank first read is polled past.
    let (page, methods) = navigation_peer(1, 1, url.clone()).await;
    navigate_exact(&page, &url, &expected, &quick_limits(1_000), &cancel)
        .await
        .unwrap();
    assert_eq!(
        *methods.lock().unwrap(),
        [
            "Page.navigate",
            "Page.navigate",
            "Runtime.evaluate",
            "Runtime.evaluate"
        ]
    );

    // Two failed loads end the navigation without reading the page.
    let (page, methods) = navigation_peer(2, 0, url.clone()).await;
    let failed = navigate_exact(&page, &url, &expected, &quick_limits(1_000), &cancel)
        .await
        .unwrap_err();
    assert!(
        failed
            .to_string()
            .contains("ChatGPT target navigation failed"),
        "{failed:#}"
    );
    assert_eq!(*methods.lock().unwrap(), ["Page.navigate", "Page.navigate"]);

    // A page that never reaches the destination fails at the command deadline.
    let (page, methods) = navigation_peer(0, usize::MAX, url.clone()).await;
    let stalled = tokio::time::timeout(
        Duration::from_secs(3),
        navigate_exact(&page, &url, &expected, &quick_limits(150), &cancel),
    )
    .await
    .expect("navigation wait is bounded")
    .unwrap_err();
    assert!(
        stalled
            .to_string()
            .contains("Owned target did not reach its exact provider navigation"),
        "{stalled:#}"
    );
    let methods = methods.lock().unwrap();
    assert_eq!(methods.iter().filter(|m| *m == "Page.navigate").count(), 1);
    assert!(methods.iter().filter(|m| *m == "Runtime.evaluate").count() >= 2);
}

#[test]
fn image_answer_text_appends_delivery_summary() {
    let source = "PUBLIC-answer with images";
    let suffix = "\n\nPUBLIC-suffix";
    let cases = [
        (json!({}), Some(source.to_owned())),
        (
            json!({"imageCount":0,"markdownSuffix":suffix}),
            Some(source.to_owned()),
        ),
        (
            json!({"imageCount":2,"markdownSuffix":suffix,"buttonDownloads":true}),
            Some(format!("{source}{suffix}")),
        ),
        (
            json!({"imageCount":2,"markdownSuffix":suffix}),
            Some(format!(
                "{}{suffix}",
                browser::normalise_image_answer(source, 2)
            )),
        ),
        (json!({"imageCount":1}), None),
    ];
    for (image, expected) in cases {
        match expected {
            Some(text) => assert_eq!(image_answer_text(source, &image).unwrap(), text, "{image}"),
            None => {
                let error = image_answer_text(source, &image).unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains("Generated images have no delivery summary"),
                    "{error:#}"
                );
            }
        }
    }
}

#[tokio::test]
async fn observe_retirement_reads_answers_and_refuses_unreadable_reports() {
    let timeout = Duration::from_millis(300);
    let answer = json!({"text":"PUBLIC-answer","completionVisible":true});
    let page_answer = answer.clone();
    let (page, _task) = peer(Duration::from_secs(1), move |request| {
        match request.method.as_str() {
            "Runtime.evaluate" => Reply::value(page_answer.clone()),
            _ => Reply::Error("PUBLIC-unavailable".into()),
        }
    })
    .await;
    let observed = observe_retirement(
        &page,
        &json!({}),
        &json!({"retirementId":"PUBLIC-marker"}),
        &json!({"kind":"session"}),
        timeout,
    )
    .await
    .unwrap();
    assert_eq!(observed, answer);

    let (page, _task) = peer(Duration::from_secs(1), |request| {
        match request.method.as_str() {
            "Runtime.evaluate" => Reply::thrown("Error: PUBLIC-report read failed"),
            _ => Reply::Error("PUBLIC-unavailable".into()),
        }
    })
    .await;
    let report = tokio::time::timeout(
        Duration::from_secs(3),
        observe_retirement(
            &page,
            &json!({}),
            &json!({"retirementId":"PUBLIC-marker"}),
            &json!({"kind":"research-report"}),
            timeout,
        ),
    )
    .await
    .expect("report observation is bounded");
    assert!(report.is_err());

    // A research commitment is never retired against unbound report evidence.
    assert!(
        require_retirement_content(&json!({}), &json!({}), &json!({"kind":"research-report"}))
            .is_err()
    );
}
