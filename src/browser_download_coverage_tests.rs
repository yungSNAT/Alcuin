use super::*;
use crate::cdp::fake::{self, Reply};
use std::sync::{Arc, Mutex};

type Calls = Arc<Mutex<Vec<(String, Value)>>>;

const FRAME: &str = "PUBLIC-frame";
const GUID: &str = "00000000-0000-4000-8000-0000000000a1";
const OTHER: &str = "00000000-0000-4000-8000-0000000000b2";
const CLICK: &str = "/* alcuin-dom:PUBLIC-click */ (cfg) => cfg";

fn begin_event(guid: &str, frame: &str) -> Value {
    json!({"method": "Browser.downloadWillBegin", "params": {
        "frameId": frame, "guid": guid, "suggestedFilename": "PUBLIC-report.txt"
    }})
}

fn progress_event(guid: &str, state: &str) -> Value {
    json!({"method": "Browser.downloadProgress", "params": {"guid": guid, "state": state}})
}

/// A scripted browser/page: records every command, acknowledges the click script once, emits
/// `on_click` events synchronously and `delayed` 50 ms after the click.
async fn scripted(
    sender: broadcast::Sender<Value>,
    on_click: Vec<Value>,
    delayed: Option<Value>,
) -> (Cdp, Calls) {
    let calls: Calls = Arc::new(Mutex::new(vec![]));
    let seen = calls.clone();
    let (cdp, _task) = fake::peer(Duration::from_secs(2), move |req| {
        seen.lock()
            .unwrap()
            .push((req.method.clone(), req.params.clone()));
        if req.dom() == Some("PUBLIC-click") {
            for event in &on_click {
                let _ = sender.send(event.clone());
            }
            if let Some(event) = delayed.clone() {
                let later = sender.clone();
                drop(tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let _ = later.send(event);
                }));
            }
            Reply::value(json!({"clicked": 1}))
        } else {
            Reply::ok()
        }
    })
    .await;
    (cdp, calls)
}

fn owner_in(
    home: &Path,
    browser: Cdp,
    events: broadcast::Receiver<Value>,
    context: Value,
) -> DownloadOwner {
    let artifacts = home.join("artifacts");
    std::fs::create_dir_all(&artifacts).unwrap();
    let directory = artifacts.join(".alcuin-browser-download-PUBLIC");
    private_dir(&directory).unwrap();
    let stage = NativeEntry::open(&directory, true).unwrap();
    let marker_path = home.join("PUBLIC-marker.json");
    let marker =
        NativeEntry::from_handle(private_file(&marker_path, true).unwrap(), false).unwrap();
    let lock = DirectoryLock::acquire(
        &home.join(".alcuin-browser-download-mutex"),
        Duration::from_secs(1),
    )
    .unwrap();
    DownloadOwner {
        browser,
        events,
        frame: FRAME.into(),
        target: "PUBLIC-target".into(),
        context,
        directory,
        stage,
        marker_path,
        marker,
        parents: vec![],
        lock: Some(lock),
        rows: BTreeMap::new(),
        files: vec![],
        published: BTreeSet::new(),
        configured: true,
        pending: false,
        invalid: false,
    }
}

fn calls_named(calls: &Calls, method: &str) -> Vec<Value> {
    calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(m, _)| m == method)
        .map(|(_, p)| p.clone())
        .collect()
}

#[tokio::test]
async fn events_only_record_armed_exact_frame_guids_and_quarantine_malformed_ones() {
    let home = tempfile::tempdir().unwrap();
    let (sender, events) = broadcast::channel(16);
    let (cdp, _calls) = scripted(sender, vec![], None).await;
    let mut owner = owner_in(home.path(), cdp.clone(), events, Value::Null);

    assert_eq!(
        owner.params(json!({"behavior": "PUBLIC"})),
        json!({"behavior": "PUBLIC"})
    );

    owner.event(begin_event(GUID, FRAME), false);
    owner.event(begin_event(GUID, "PUBLIC-other-frame"), true);
    owner.event(progress_event(GUID, "completed"), true);
    assert!(owner.rows.is_empty());
    assert!(!owner.invalid);

    owner.event(begin_event(GUID, FRAME), true);
    assert_eq!(owner.rows[GUID]["state"], "inProgress");
    assert_eq!(owner.rows[GUID]["targetId"], "PUBLIC-target");
    owner.event(progress_event(GUID, "interrupted"), true);
    assert_eq!(owner.rows[GUID]["state"], "inProgress");

    owner.event(begin_event("PUBLIC-not-a-guid", FRAME), true);
    assert!(owner.invalid);
    assert_eq!(owner.rows.len(), 1);

    owner.invalid = false;
    owner.event(begin_event(GUID, FRAME), true);
    assert!(owner.invalid, "duplicate GUID must quarantine");

    let err = owner
        .capture(
            &cdp,
            CLICK,
            json!({}),
            Duration::from_millis(200),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("ownership is uncertain"),
        "{err:#}"
    );

    let marker = owner.marker_path.clone();
    let err = owner.finish(Duration::from_millis(100)).await.unwrap_err();
    assert!(err.to_string().contains("remains quarantined"), "{err:#}");
    assert!(marker.exists(), "quarantined marker must be preserved");
}

struct CaptureCase {
    capacity: usize,
    before: Vec<Value>,
    on_click: Vec<Value>,
    expected: &'static str,
    pending: bool,
}

#[tokio::test]
async fn capture_failures_report_the_reason_and_retain_ownership() {
    let cases = vec![
        CaptureCase {
            capacity: 1,
            before: vec![progress_event(OTHER, "completed"); 3],
            on_click: vec![],
            expected: "Browser download event history is incomplete",
            pending: false,
        },
        CaptureCase {
            capacity: 1,
            before: vec![],
            on_click: vec![progress_event(OTHER, "completed"); 3],
            expected: "Owned browser download event history was lost",
            pending: true,
        },
        CaptureCase {
            capacity: 16,
            before: vec![],
            on_click: vec![begin_event(GUID, FRAME), progress_event(GUID, "canceled")],
            expected: "Owned browser download was cancelled",
            pending: true,
        },
        CaptureCase {
            capacity: 16,
            before: vec![],
            on_click: vec![begin_event("PUBLIC-not-a-guid", FRAME)],
            expected: "duplicated or malformed",
            pending: true,
        },
        CaptureCase {
            capacity: 16,
            before: vec![],
            on_click: vec![begin_event(GUID, FRAME), begin_event(OTHER, FRAME)],
            expected: "ambiguous downloads",
            pending: true,
        },
    ];
    for case in cases {
        let home = tempfile::tempdir().unwrap();
        let (sender, events) = broadcast::channel(case.capacity);
        let (cdp, _calls) = scripted(sender.clone(), case.on_click, None).await;
        let mut owner = owner_in(home.path(), cdp.clone(), events, Value::Null);
        for event in case.before {
            sender.send(event).unwrap();
        }
        let err = owner
            .capture(
                &cdp,
                CLICK,
                json!({"PUBLIC": "cfg"}),
                Duration::from_millis(500),
                &CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains(case.expected),
            "{}: {err:#}",
            case.expected
        );
        assert_eq!(owner.pending, case.pending, "{}", case.expected);
    }
}

#[tokio::test]
async fn capture_waits_for_completion_then_finish_retires_published_download() {
    let home = tempfile::tempdir().unwrap();
    let (sender, events) = broadcast::channel(16);
    let (cdp, calls) = scripted(
        sender.clone(),
        vec![begin_event(GUID, FRAME)],
        Some(progress_event(GUID, "completed")),
    )
    .await;
    let mut owner = owner_in(home.path(), cdp.clone(), events, Value::Null);
    let stage = owner.stage_path().to_owned();
    std::fs::write(stage.join(GUID), b"PUBLIC-bytes").unwrap();
    // Notifications before arming are discarded and never establish ownership.
    sender.send(begin_event(OTHER, FRAME)).unwrap();

    let (bytes, row) = owner
        .capture(
            &cdp,
            CLICK,
            json!({"PUBLIC": "cfg"}),
            Duration::from_secs(2),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(bytes, b"PUBLIC-bytes");
    assert_eq!(row["guid"], GUID);
    assert_eq!(row["state"], "completed");
    assert_eq!(row["suggestedFilename"], "PUBLIC-report.txt");
    assert_eq!(
        owner.rows.keys().map(String::as_str).collect::<Vec<_>>(),
        vec![GUID]
    );
    assert_eq!(owner.rows[GUID]["capturedSha256"], json!(sha256(&bytes)));
    assert_eq!(owner.rows[GUID]["capturedSize"], json!(bytes.len()));
    assert!(!owner.pending);
    assert_eq!(owner.files.len(), 1);

    let err = owner
        .publication_confirmed(GUID, &json!({"sha256": "PUBLIC-wrong", "sizeBytes": 12}))
        .unwrap_err();
    assert!(err.to_string().contains("differs"), "{err:#}");
    let err = owner.discard_non_image(OTHER, &bytes).unwrap_err();
    assert!(err.to_string().contains("unowned download GUID"), "{err:#}");
    let err = owner
        .discard_non_image(GUID, b"\x89PNG\r\n\x1a\nPUBLIC")
        .unwrap_err();
    assert!(err.to_string().contains("cannot be discarded"), "{err:#}");
    owner.discard_non_image(GUID, &bytes).unwrap();
    assert!(owner.published.contains(GUID));

    let marker = owner.marker_path.clone();
    let cleanup = owner.finish(Duration::from_secs(1)).await.unwrap();
    assert!(cleanup.is_none(), "{cleanup:?}");
    assert!(!marker.exists());
    assert!(!stage.exists());
    assert_eq!(
        calls_named(&calls, "Browser.setDownloadBehavior"),
        vec![json!({"behavior": "default", "eventsEnabled": false})]
    );
    assert!(calls_named(&calls, "Browser.cancelDownload").is_empty());
}

#[tokio::test]
async fn finish_cancels_unsettled_downloads_in_the_owned_context() {
    let home = tempfile::tempdir().unwrap();
    let (sender, events) = broadcast::channel(16);
    let (cdp, calls) = scripted(sender.clone(), vec![], None).await;
    let mut owner = owner_in(home.path(), cdp, events, json!("PUBLIC-context"));
    assert_eq!(
        owner.params(json!({"guid": GUID})),
        json!({"guid": GUID, "browserContextId": "PUBLIC-context"})
    );
    owner
        .rows
        .insert(GUID.into(), json!({"guid": GUID, "state": "inProgress"}));
    sender.send(progress_event(GUID, "canceled")).unwrap();

    let marker = owner.marker_path.clone();
    let cleanup = owner.finish(Duration::from_secs(1)).await.unwrap();
    assert!(cleanup.is_none(), "{cleanup:?}");
    assert!(!marker.exists());
    assert_eq!(
        calls_named(&calls, "Browser.cancelDownload"),
        vec![json!({"guid": GUID, "browserContextId": "PUBLIC-context"})]
    );
    assert_eq!(
        calls_named(&calls, "Browser.setDownloadBehavior"),
        vec![
            json!({"behavior": "default", "eventsEnabled": false, "browserContextId": "PUBLIC-context"})
        ]
    );
}

#[test]
fn sandbox_download_urls_are_mapped_and_unsafe_paths_rejected() {
    for (raw, path) in [
        (
            "sandbox:/mnt/data/PUBLIC report.txt",
            "/mnt/data/PUBLIC report.txt",
        ),
        (
            "https://chatgpt.com/backend-api/sandbox/download?path=/mnt/data/PUBLIC.csv",
            "/mnt/data/PUBLIC.csv",
        ),
    ] {
        let url = download_url(raw).unwrap();
        assert_eq!(url.host_str(), Some("chatgpt.com"), "{raw}");
        assert_eq!(url.path(), "/backend-api/sandbox/download", "{raw}");
        let query = url
            .query_pairs()
            .find(|(key, _)| key == "path")
            .map(|(_, value)| value.into_owned());
        assert_eq!(query.as_deref(), Some(path), "{raw}");
    }
    for (raw, expected) in [
        ("sandbox:/etc/PUBLIC", "Unsafe sandbox download path"),
        (
            "sandbox:/mnt/data/../PUBLIC",
            "Unsafe sandbox download path",
        ),
        (
            "sandbox:/mnt/data/PUBLIC\\x",
            "Unsafe sandbox download path",
        ),
        ("sandbox:/mnt/data/PUBLIC\0", "Unsafe sandbox download path"),
        (
            "sandbox:/mnt/data/%2E%2E/PUBLIC",
            "Unsafe sandbox download path",
        ),
        (
            "sandbox:/mnt/data/PUBLIC%5cx",
            "Unsafe sandbox download path",
        ),
        (
            "https://chatgpt.com/backend-api/sandbox/download?path=/tmp/PUBLIC",
            "not a maintained provider download route",
        ),
        (
            "https://chatgpt.com/backend-api/sandbox/download?path=/mnt/data/%2e%2e/PUBLIC",
            "not a maintained provider download route",
        ),
        (
            "https://chatgpt.com/backend-api/sandbox/download",
            "not a maintained provider download route",
        ),
    ] {
        let err = download_url(raw).unwrap_err();
        assert!(err.to_string().contains(expected), "{raw}: {err:#}");
    }
}

#[test]
fn image_signatures_are_detected_for_every_supported_format() {
    let cases: &[(&[u8], Option<&str>)] = &[
        (b"\x89PNG\r\n\x1a\nPUBLIC", Some("png")),
        (&[0xff, 0xd8, 0xff, 0x00], Some("jpg")),
        (b"RIFF\x00\x00\x00\x00WEBPVP8 ", Some("webp")),
        (b"RIFF\x00\x00\x00\x00WAVE", None),
        (b"GIF87aPUBLIC", Some("gif")),
        (b"GIF89aPUBLIC", Some("gif")),
        (
            b"  <svg xmlns=\"http://www.w3.org/2000/svg\"></svg>",
            Some("svg"),
        ),
        (b"<SVG>PUBLIC</SVG>", Some("svg")),
        (b"<?xml version=\"1.0\"?>\n<svg width=\"1\"/>", Some("svg")),
        (b"<?xml version=\"1.0\" PUBLIC", None),
        (b"<svgfoo>", None),
        (b"<svg", None),
        (b"PUBLIC text", None),
    ];
    for (bytes, expected) in cases {
        assert_eq!(
            image_type(bytes).map(|(ext, _)| ext),
            *expected,
            "{}",
            String::from_utf8_lossy(bytes)
        );
    }
    assert_eq!(image_type(b"<svg>"), Some(("svg", "image/svg+xml")));
    assert_eq!(image_type(b"GIF89a"), Some(("gif", "image/gif")));
}

#[test]
fn published_images_get_fallback_names_and_unique_destinations() {
    let home = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    let gif = b"GIF89a PUBLIC image";

    let err = publish_download(b"PUBLIC text", home.path(), "x.png", true, &cancel).unwrap_err();
    assert!(
        err.to_string().contains("no verified image signature"),
        "{err:#}"
    );

    let first = publish_download(gif, home.path(), "...", true, &cancel).unwrap();
    assert_eq!(first["kind"], "image");
    assert_eq!(first["mimeType"], "image/gif");
    assert_eq!(
        first["validation"],
        json!({"type": "image-signature", "ok": true})
    );
    assert_eq!(first["sizeBytes"], json!(gif.len()));
    assert!(
        first["path"].as_str().unwrap().ends_with("download.bin"),
        "{first}"
    );

    let second = publish_download(gif, home.path(), "...", true, &cancel).unwrap();
    assert!(
        second["path"]
            .as_str()
            .unwrap()
            .ends_with("download (1).bin"),
        "{second}"
    );
    assert_eq!(
        std::fs::read(home.path().join("download.bin")).unwrap(),
        gif
    );
    assert_eq!(
        std::fs::read(home.path().join("download (1).bin")).unwrap(),
        gif
    );

    let named = publish_download(gif, home.path(), "PUBLIC<pic>", true, &cancel).unwrap();
    assert!(
        named["path"].as_str().unwrap().ends_with("PUBLIC_pic_.gif"),
        "{named}"
    );
}

#[test]
fn publishing_fails_when_every_candidate_destination_exists() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("PUBLIC-full.txt"), b"").unwrap();
    for index in 1..1000 {
        std::fs::write(home.path().join(format!("PUBLIC-full ({index}).txt")), b"").unwrap();
    }
    let err = publish_download(
        b"PUBLIC data",
        home.path(),
        "PUBLIC-full.txt",
        false,
        &CancellationToken::new(),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("No unused artifact destination"),
        "{err:#}"
    );
    assert!(
        std::fs::read(home.path().join("PUBLIC-full.txt"))
            .unwrap()
            .is_empty()
    );
    assert!(
        std::fs::read(home.path().join("PUBLIC-full (999).txt"))
            .unwrap()
            .is_empty()
    );
}
