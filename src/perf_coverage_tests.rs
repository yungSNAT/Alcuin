use super::*;
use serde_json::json;

fn phase_event(phase: &str, turn_index: u64, duration_ms: f64) -> Value {
    json!({
        "name": format!("consultation.{phase}"),
        "data": {"turnIndex": turn_index, "durationMs": duration_ms, "ok": 1}
    })
}

fn complete_events(queue_ms: f64) -> Vec<Value> {
    PHASES
        .into_iter()
        .map(|phase| phase_event(phase, 0, if phase == "queue" { queue_ms } else { 10.0 }))
        .collect()
}

fn write_trace(path: &Path, events: &[Value]) {
    let bytes = serde_json::to_vec(&json!({"complete": true, "events": events})).unwrap();
    fs::write(path, bytes).unwrap();
}

#[test]
fn summary_ignores_unnamed_events_and_preserves_fractional_timings() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("PUBLIC-first.json");
    let second = dir.path().join("PUBLIC-second.json");
    let mut events = vec![
        json!("PUBLIC-not-an-object"),
        json!({"data": "PUBLIC-no-name"}),
        json!({"name": 42}),
    ];
    events.extend(complete_events(1.0));
    write_trace(&first, &events);
    write_trace(&second, &complete_events(2.5));

    let summary = summarise(&[first, second]).unwrap();
    assert_eq!(summary.completed_files, 2);
    assert_eq!(summary.incomplete_files, 0);
    assert_eq!(
        serde_json::to_string(&summary.phases[0]).unwrap(),
        r#"{"phase":"queue","n":2,"medianMs":1.75,"p95Ms":2.5}"#
    );
}

#[test]
fn summary_rejects_directory_input_with_its_index() {
    let dir = tempfile::tempdir().unwrap();
    let valid = dir.path().join("PUBLIC-valid.json");
    let directory_input = dir.path().join("PUBLIC-directory");
    write_trace(&valid, &complete_events(1.0));
    fs::create_dir(&directory_input).unwrap();

    let error = summarise(&[valid, directory_input]).unwrap_err();
    assert_eq!(
        error.to_string(),
        "perf-summary input 2: regular file required."
    );
}

#[test]
fn summary_rejects_invalid_detail_timings_and_overflowing_detail_totals() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("PUBLIC-detail.json");
    let turn_detail = "consultation-detail.composer-readiness";

    for (name, data) in [
        (
            turn_detail,
            json!({"detail": 1, "durationMs": -0.25, "ok": 1, "turnIndex": 0}),
        ),
        (
            turn_detail,
            json!({"detail": 1, "durationMs": 0.25, "ok": 2, "turnIndex": 0}),
        ),
        (
            turn_detail,
            json!({"detail": 1, "durationMs": 0.25, "ok": 1}),
        ),
        (
            "consultation-detail.output-metadata-persistence",
            json!({"detail": 1, "durationMs": 0.25, "ok": 1, "turnIndex": null}),
        ),
    ] {
        let mut events = complete_events(1.0);
        events.push(json!({"name": name, "data": data}));
        write_trace(&path, &events);

        let error = summarise(std::slice::from_ref(&path)).unwrap_err();
        assert_eq!(
            error.to_string(),
            "perf-summary input 1: invalid detail timing.",
            "{name}: {data}"
        );
    }

    let large_segment = json!({
        "name": turn_detail,
        "data": {"detail": 1, "durationMs": 1.0e308, "ok": 1, "turnIndex": 0}
    });
    let mut events = complete_events(1.0);
    events.push(large_segment.clone());
    write_trace(&path, &events);
    assert_eq!(
        summarise(std::slice::from_ref(&path))
            .unwrap()
            .completed_files,
        1
    );

    events.push(large_segment);
    write_trace(&path, &events);
    let error = summarise(&[path]).unwrap_err();
    assert_eq!(
        error.to_string(),
        "perf-summary input 1: detail total overflow."
    );
}

#[test]
fn summary_excludes_traces_with_misordered_turns_or_uploads() {
    let dir = tempfile::tempdir().unwrap();
    let later_turn = |turn_index| {
        PHASES[2..]
            .iter()
            .map(move |phase| phase_event(phase, turn_index, 1.0))
    };

    let mut skipped_turn = complete_events(1.0);
    skipped_turn.extend(later_turn(2));

    let mut premature_turn = complete_events(1.0);
    premature_turn.truncate(5);
    premature_turn.extend(later_turn(1));

    let mut early_upload = complete_events(1.0);
    early_upload.insert(2, phase_event("upload", 0, 1.0));

    for (filename, events) in [
        ("PUBLIC-skipped-turn.json", skipped_turn),
        ("PUBLIC-premature-turn.json", premature_turn),
        ("PUBLIC-early-upload.json", early_upload),
    ] {
        let path = dir.path().join(filename);
        write_trace(&path, &events);

        let summary = summarise(&[path]).unwrap();
        assert_eq!(
            (summary.completed_files, summary.incomplete_files),
            (0, 1),
            "{filename}"
        );
        assert!(
            summary
                .phases
                .iter()
                .all(|row| { row.n == 0 && row.median_ms.is_none() && row.p95_ms.is_none() }),
            "{filename}"
        );
    }
}

#[test]
fn equals_form_trace_arguments_select_the_default_or_explicit_path() {
    for (input, expected, selected) in [
        (
            ["--perf-trace=PUBLIC-first.json", "--perf-trace="],
            &[
                "--perf-trace",
                "--perf-trace-path",
                "PUBLIC-first.json",
                "--perf-trace",
            ][..],
            "1",
        ),
        (
            ["--perf-trace-path=PUBLIC-override.json", "--perf-trace"],
            &["--perf-trace-path=PUBLIC-override.json", "--perf-trace"][..],
            "PUBLIC-override.json",
        ),
    ] {
        let (args, value, error) = normalise_args(&input.map(str::to_owned));
        assert_eq!(
            args.iter().map(String::as_str).collect::<Vec<_>>(),
            expected
        );
        assert_eq!(value.as_deref(), Some(selected));
        assert_eq!(error, None);
    }
}

struct RestoreTrace(Option<Writer>);

impl Drop for RestoreTrace {
    fn drop(&mut self) {
        *TRACE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = self.0.take();
    }
}

#[test]
fn detached_options_override_environment_and_failed_flush_is_not_retried() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("PUBLIC-blocker");
    let path = blocker.join("PUBLIC-trace.json");
    fs::write(&blocker, b"PUBLIC-obstruction").unwrap();

    let _restore = RestoreTrace(TRACE.lock().unwrap().take());
    start(
        Some("PUBLIC-blocker/PUBLIC-trace.json".into()),
        &[],
        dir.path(),
    );

    let environment = Some("PUBLIC-environment.json");
    let session = "PUBLIC-session/one";
    assert_eq!(
        detached_value(environment, session).as_deref(),
        Some("PUBLIC-blocker/PUBLIC-trace.PUBLIC-session_one.json")
    );
    // Set the stored option directly so the default-mode case does not depend on the environment.
    with(|trace| trace.option = Some("1".into())).unwrap();
    assert_eq!(detached_value(environment, session).as_deref(), Some("1"));

    flush(7);
    assert_eq!(
        fs::read(&blocker).unwrap(),
        b"PUBLIC-obstruction".as_slice()
    );
    let (flushed, checkpoints, events) =
        with(|trace| (trace.flushed, trace.count, trace.events.clone())).unwrap();
    assert!(flushed);
    assert_eq!(checkpoints, 1);
    let exit = events.iter().find(|event| event["name"] == "exit").unwrap();
    assert_eq!(exit["data"]["exitCode"], 7);

    fs::remove_file(&blocker).unwrap();
    fs::create_dir(&blocker).unwrap();
    flush(8);
    assert!(
        !path.exists(),
        "a second flush must not retry the failed write"
    );
    assert_eq!(with(|trace| trace.count), Some(1));
}
