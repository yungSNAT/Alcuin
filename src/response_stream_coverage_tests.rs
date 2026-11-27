use super::*;

fn accumulator_with(items: Value) -> ResponseAccumulator {
    let mut accumulator = ResponseAccumulator::default();
    accumulator
        .event(&json!({"type":"response.created","response":{"id":"resp_public","status":"in_progress","output":[]}}))
        .unwrap();
    for (index, item) in items.as_array().unwrap().iter().enumerate() {
        accumulator
            .event(&json!({"type":"response.output_item.added","output_index":index,"item":item}))
            .unwrap();
    }
    accumulator
}

#[test]
fn sse_decoder_ignores_unknown_fields_and_bare_unknown_lines() {
    let mut decoder = SseDecoder::default();
    let events = decoder
        .push(b"id: PUBLIC-1\nretry: 100\nevent: response.created\nunknown\ndata: {}\n\n")
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "response.created");
    assert_eq!(events[0].data, "{}");
    // An event block with only unknown fields dispatches nothing.
    let events = decoder.push(b"id: PUBLIC-2\nretry: 5\n\n").unwrap();
    assert!(events.is_empty());
    assert!(decoder.finish().unwrap().is_empty());
}

#[test]
fn reasoning_text_streams_into_reasoning_content_without_caller_delta() {
    let mut accumulator = accumulator_with(json!([{"id":"rs_a","type":"reasoning"}]));
    accumulator
        .event(&json!({"type":"response.content_part.added","item_id":"rs_a","output_index":0,"content_index":0,"part":{"type":"reasoning_text","text":""}}))
        .unwrap();
    let delta = accumulator
        .event(&json!({"type":"response.reasoning_text.delta","item_id":"rs_a","output_index":0,"content_index":0,"delta":"PUBLIC think"}))
        .unwrap();
    assert_eq!(delta, None);
    assert_eq!(
        accumulator.snapshot.as_ref().unwrap()["output"][0]["content"][0]["text"],
        "PUBLIC think"
    );
    accumulator
        .event(&json!({"type":"response.reasoning_text.done","item_id":"rs_a","output_index":0,"content_index":0,"text":"PUBLIC thought."}))
        .unwrap();
    let result = accumulator.finish().unwrap();
    assert_eq!(
        result["output"][0]["content"],
        json!([{"type":"reasoning_text","text":"PUBLIC thought."}])
    );
    // Reasoning text is not answer text.
    assert_eq!(result["output_text"], "");
}

#[test]
fn status_events_update_items_with_and_without_item_identity() {
    let mut accumulator = accumulator_with(json!([
        {"id":"ws_a","type":"web_search_call","status":"in_progress"},
        {"id":"ws_b","type":"web_search_call","status":"searching"},
    ]));
    assert_eq!(
        accumulator
            .event(
                &json!({"type":"response.web_search_call.failed","item_id":"ws_a","output_index":0})
            )
            .unwrap(),
        None
    );
    assert_eq!(
        accumulator
            .event(&json!({"type":"response.web_search_call.in_progress","output_index":1}))
            .unwrap(),
        None
    );
    let snapshot = accumulator.snapshot.as_ref().unwrap();
    assert_eq!(snapshot["output"][0]["status"], "failed");
    assert_eq!(snapshot["output"][1]["status"], "in_progress");
}

#[test]
fn missing_part_indices_are_rejected_without_changing_the_snapshot() {
    let items = json!([
        {"id":"msg_a","type":"message","content":[{"type":"output_text","text":"","annotations":[]}]},
        {"id":"rs_a","type":"reasoning","summary":[{"type":"summary_text","text":""}]},
    ]);
    for event in [
        json!({"type":"response.content_part.added","item_id":"msg_a","output_index":0,"part":{"type":"output_text","text":"","annotations":[]}}),
        json!({"type":"response.reasoning_summary_part.added","item_id":"rs_a","output_index":1,"part":{"type":"summary_text","text":""}}),
        json!({"type":"response.output_text.delta","item_id":"msg_a","output_index":0,"delta":"PUBLIC x"}),
        json!({"type":"response.reasoning_summary_text.delta","item_id":"rs_a","output_index":1,"delta":"PUBLIC y"}),
    ] {
        let mut accumulator = accumulator_with(items.clone());
        let before = accumulator.snapshot.clone();
        let err = accumulator.event(&event).unwrap_err();
        assert_eq!(err.to_string(), "Invalid streamed array index", "{event}");
        assert_eq!(accumulator.snapshot, before, "{event}");
    }
}

#[test]
fn canonical_text_without_snapshot_leaves_accumulator_empty() {
    let mut accumulator = ResponseAccumulator::default();
    accumulator.canonical_text();
    assert!(accumulator.snapshot.is_none());
    let err = accumulator.finish().unwrap_err();
    assert_eq!(err.to_string(), "Stream ended without producing a Response");
}
