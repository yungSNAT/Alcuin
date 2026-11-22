use super::*;
use crate::cdp::fake::{Reply, peer};

/// Runs `configure` against a scripted window peer and returns its result plus every
/// command sent, reduced to `[method, params]` pairs.
async fn run_configure(config: Value, initial: Value, placed: Value) -> (Result<()>, Vec<Value>) {
    let home = tempfile::tempdir().unwrap();
    let profile = ProfileRegistry::open(home.path()).unwrap();
    let (page, task) = peer(Duration::from_secs(1), move |request| {
        match request.method.as_str() {
            "Browser.getWindowForTarget" => {
                Reply::Result(json!({"windowId": 7, "bounds": initial.clone()}))
            }
            "Browser.getWindowBounds" => Reply::Result(json!({"bounds": placed.clone()})),
            "Browser.setWindowBounds" => Reply::ok(),
            _ => Reply::Error("PUBLIC-unexpected".into()),
        }
    })
    .await;
    let result = configure(
        &page,
        &profile,
        &config,
        Duration::from_secs(1),
        &CancellationToken::new(),
    )
    .await;
    page.disconnect_and_wait(Duration::from_secs(1))
        .await
        .unwrap();
    let trace = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    let calls = trace
        .iter()
        .map(|message| json!([message["method"], message["params"]]))
        .collect();
    (result, calls)
}

#[test]
fn normal_window_satisfaction_compares_only_requested_fields() {
    let target = Bounds::parse(&json!({"left": 10, "top": 20})).unwrap();
    for (current, expected) in [
        (
            json!({"left": 10, "top": 20, "width": 300, "height": 400}),
            true,
        ),
        (json!({"left": 10, "top": 20}), true),
        (
            json!({"left": 11, "top": 20, "width": 300, "height": 400}),
            false,
        ),
        (json!({"left": 10, "top": 21}), false),
        (json!({"left": 10}), false),
        (
            json!({"left": 10, "top": 20, "windowState": "maximized"}),
            false,
        ),
    ] {
        let current = Bounds::parse(&current).unwrap();
        assert_eq!(target.satisfied(&current), expected, "{current:?}");
    }
    let anything = Bounds::parse(&json!({})).unwrap();
    let current = Bounds::parse(&json!({"left": -5, "width": 9})).unwrap();
    assert!(anything.satisfied(&current));
    let sized = Bounds::parse(&json!({"width": 300, "height": 400})).unwrap();
    assert!(!sized.satisfied(&Bounds::parse(&json!({"width": 300, "height": 401})).unwrap()));
}

#[test]
fn hidden_launch_without_preferences_saves_default_placement() {
    let home = tempfile::tempdir().unwrap();
    let profile = ProfileRegistry::open(home.path()).unwrap();
    prepare_hidden_launch(&profile).unwrap();
    let written: Value =
        serde_json::from_slice(&std::fs::read(home.path().join(SAVED_STATE)).unwrap()).unwrap();
    assert_eq!(
        written,
        json!({"version": 1, "bounds": {"left": 80, "top": 80, "width": 1280, "height": 720, "windowState": "normal"}})
    );
    let restored = saved(&profile).unwrap();
    assert_eq!(
        (restored.left, restored.top, restored.width, restored.height),
        (Some(80), Some(80), Some(1280), Some(720))
    );
    assert_eq!(restored.window_state, "normal");
}

#[test]
fn hidden_launch_reports_failure_when_saved_state_cannot_be_written() {
    let home = tempfile::tempdir().unwrap();
    let profile = ProfileRegistry::open(home.path()).unwrap();
    let blocker = home.path().join(SAVED_STATE);
    std::fs::create_dir(&blocker).unwrap();
    std::fs::write(blocker.join("PUBLIC-keep"), b"PUBLIC-fixture").unwrap();
    assert!(prepare_hidden_launch(&profile).is_err());
    assert!(blocker.is_dir());
    assert!(blocker.join("PUBLIC-keep").is_file());
    assert!(saved(&profile).is_none());
}

#[tokio::test]
async fn headless_configuration_sends_no_window_commands() {
    let (result, calls) = run_configure(
        json!({"headless": true, "hideWindow": true, "windowBounds": {"left": 1}}),
        json!({}),
        json!({}),
    )
    .await;
    result.unwrap();
    assert!(calls.is_empty(), "{calls:?}");
}

#[cfg(not(target_os = "macos"))]
#[tokio::test]
async fn hidden_window_off_macos_skips_placement() {
    let (result, calls) = run_configure(
        json!({"hideWindow": true, "windowBounds": {"left": 1, "top": 2}}),
        json!({"left": 0, "top": 0, "width": 10, "height": 10}),
        json!({}),
    )
    .await;
    result.unwrap();
    assert!(calls.is_empty(), "{calls:?}");
}

#[tokio::test]
async fn placement_sends_only_the_commands_each_target_needs() {
    let normal_current =
        json!({"left": 0, "top": 0, "width": 100, "height": 100, "windowState": "normal"});
    let cases = [
        (
            // Already in place: only the lookup is sent.
            json!({"left": 10, "top": 20, "width": 300, "height": 400}),
            json!({"left": 10, "top": 20, "width": 300, "height": 400, "windowState": "normal"}),
            json!({}),
            json!([["Browser.getWindowForTarget", {}]]),
        ),
        (
            // Maximise without coordinates: no staging rectangle, just the state change.
            json!({"windowState": "maximized"}),
            normal_current.clone(),
            json!({"left": 0, "top": 0, "width": 1920, "height": 1080, "windowState": "maximized"}),
            json!([
                ["Browser.getWindowForTarget", {}],
                ["Browser.setWindowBounds", {"windowId": 7, "bounds": {"windowState": "maximized"}}],
                ["Browser.getWindowBounds", {"windowId": 7}]
            ]),
        ),
        (
            // Normal rectangle that Chrome does not honour: placement still succeeds.
            json!({"left": 10, "top": 20, "width": 300, "height": 400}),
            normal_current.clone(),
            normal_current,
            json!([
                ["Browser.getWindowForTarget", {}],
                ["Browser.setWindowBounds", {"windowId": 7, "bounds": {"left": 10, "top": 20, "width": 300, "height": 400, "windowState": "normal"}}],
                ["Browser.getWindowBounds", {"windowId": 7}]
            ]),
        ),
    ];
    for (target, initial, placed, expected) in cases {
        let (result, calls) =
            run_configure(json!({"windowBounds": target.clone()}), initial, placed).await;
        result.unwrap();
        assert_eq!(Value::Array(calls), expected, "target {target}");
    }
}
