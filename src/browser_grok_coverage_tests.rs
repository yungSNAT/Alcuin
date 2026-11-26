use super::*;
use crate::cdp::fake::{Reply, peer};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn enter_private_rejects_an_unrecognised_observation_without_activating() {
    for observation in [
        json!({"state": "PUBLIC-unknown"}),
        json!({}),
        json!({"state": 1}),
        json!({"PUBLIC-field": "private"}),
    ] {
        let cancel = CancellationToken::new();
        let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
        let recorded = seen.clone();
        let answer = observation.clone();
        let (cdp, _peer) = peer(Duration::from_secs(2), move |request| {
            if request.method == "Runtime.evaluate" {
                recorded.lock().unwrap().push(request.arg());
                Reply::value(answer.clone())
            } else {
                Reply::ok()
            }
        })
        .await;
        let error = enter_private(&cdp, Duration::from_secs(3), &cancel)
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Initial Grok Private observation is invalid",
            "{observation}"
        );
        let args = seen.lock().unwrap().clone();
        assert_eq!(args, vec![json!({"action": "inspect"})], "{observation}");
    }
}

#[test]
fn extends_rejects_non_object_commitments_or_observations() {
    let object = json!({"state": "committed", "responseId": null, "assistantMessageId": null});
    assert!(extends(&object, &object));
    for (saved, observed) in [
        (Value::Null, object.clone()),
        (object.clone(), Value::Null),
        (object.clone(), json!([object.clone()])),
        (json!("committed"), json!("committed")),
        (json!([]), json!([])),
    ] {
        assert!(!extends(&saved, &observed), "{saved} {observed}");
    }
}
