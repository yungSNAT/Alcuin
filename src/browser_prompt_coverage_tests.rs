use super::*;
use serde_json::json;

#[test]
fn control_plan_describes_browser_focus_and_mode_specific_guidance() {
    let cases: &[(Value, &[&str])] = &[
        (
            json!({"attachRunning": true, "keepBrowser": true}),
            &[
                "[preview] Browser control: attach to an already-running local Chrome session; may focus/control the browser UI.",
                "[preview] Browser guidance: Alcuin opens a dedicated tab and leaves the existing browser process alone.",
                "[preview] Browser guidance: The browser stays open because Alcuin did not launch it.",
            ],
        ),
        (
            json!({"remoteChrome": {"host": "127.0.0.1"}, "browserTabRef": " PUBLIC-tab "}),
            &[
                "[preview] Browser control: reuse an existing remote Chrome tab; does not use a visible local browser window.",
                "[preview] Browser guidance: Alcuin reuses the matching ChatGPT tab (PUBLIC-tab) in the configured remote Chrome session.",
                "[preview] Browser guidance: Local Chrome launch, cookie copy, and window hiding flags are skipped.",
            ],
        ),
        (
            json!({"headless": true}),
            &[
                "[preview] Browser control: launch headless Chrome; does not use a visible local browser window.",
                "[preview] Browser guidance: Headless mode avoids visible UI but may be blocked by ChatGPT or Cloudflare.",
            ],
        ),
        #[cfg(not(target_os = "macos"))]
        (
            json!({"hideWindow": true}),
            &[
                "[preview] Browser control: launch visible Chrome; may focus/control the browser UI.",
                "[preview] Browser guidance: --browser-hide-window has no effect on Windows or Linux; Chrome stays visible.",
                "[preview] Browser guidance: For the calmest shared-desktop flow, prefer --browser-attach-running or --remote-chrome.",
            ],
        ),
    ];

    for (config, expected) in cases {
        assert_eq!(control_plan_lines(config, "preview"), *expected, "{config}");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn bundle_creation_removes_its_directory_when_the_file_cannot_be_created() {
    let root = tempfile::tempdir().unwrap();
    let name = format!("PUBLIC-{}.txt", "x".repeat(4096));
    let Err(error) = OwnedBundle::create(root.path(), &name, b"PUBLIC-bundle") else {
        panic!("bundle creation accepted an overlong filename");
    };

    let errno = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<std::io::Error>())
        .and_then(std::io::Error::raw_os_error);
    assert_eq!(errno, Some(libc::ENAMETOOLONG), "{error:#}");
    assert!(fs::read_dir(root.path()).unwrap().next().is_none());
}

#[test]
fn fallback_reports_colliding_basenames_in_utf16_path_order_when_display_paths_tie() {
    let root = tempfile::tempdir().unwrap();
    // U+10000 sorts before U+E000 in UTF-16, but after it in UTF-8.
    let first = root.path().join("PUBLIC-\u{10000}/PUBLIC  image.png");
    let second = root.path().join("PUBLIC-\u{e000}/public image.PNG");
    let bytes = b"PUBLIC-image-fixture";
    let files = [&second, &first].map(|path| {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
        PromptFile {
            path: path.to_path_buf(),
            display_path: "PUBLIC-tied-display-path".into(),
            content: String::new(),
            size_bytes: bytes.len() as u64,
            scanned_sha256: None,
        }
    });

    let Err(error) = BrowserPrompt::remote_fallback(&files, "PUBLIC-review", None, root.path())
    else {
        panic!("fallback accepted renderer-equivalent basenames");
    };
    assert_eq!(
        error.to_string(),
        format!(
            "Browser upload cannot safely include multiple files named \"PUBLIC  image.png\":\n\
             - {}\n\
             - {}\n\
             Use --browser-bundle-files (and --browser-bundle-format zip for raw or binary files), or choose unique names before retrying.",
            first.display(),
            second.display()
        )
    );
}

#[test]
fn bundle_validation_rejects_unknown_formats_and_enforces_the_attachment_limit() {
    let root = tempfile::tempdir().unwrap();
    let Err(error) = BrowserPrompt::assemble(
        &[],
        "PUBLIC-review",
        None,
        &json!({"browserBundleFormat": "PUBLIC-unknown"}),
        root.path(),
    ) else {
        panic!("assembly accepted an unknown bundle format");
    };
    assert_eq!(
        error.to_string(),
        "Browser bundle format must be auto, text or zip."
    );

    for (format, label) in [(BundleFormat::Zip, "zip"), (BundleFormat::Text, "text")] {
        assert!(count(10, format).is_ok());
        assert_eq!(
            count(11, format).unwrap_err().to_string(),
            format!(
                "Browser upload has 11 attachments after applying bundle format \"{label}\". Use --browser-bundle-format auto or zip to stay within the 10-attachment limit."
            )
        );
    }
}

#[test]
fn fallback_preserves_a_blank_composer_unless_a_pending_zip_adds_instructions() {
    let root = tempfile::tempdir().unwrap();
    let display_path = "PUBLIC-assets/PUBLIC-image.png";
    let path = root.path().join(display_path);
    let bytes = b"PUBLIC-image-fixture";
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, bytes).unwrap();
    let files = [PromptFile {
        path: path.clone(),
        display_path: display_path.into(),
        content: String::new(),
        size_bytes: bytes.len() as u64,
        scanned_sha256: None,
    }];
    let composer = " \t\n ";

    let unbundled = BrowserPrompt::remote_fallback(&files, composer, None, root.path()).unwrap();
    assert_eq!(unbundled.composer_text, composer);
    assert_eq!(unbundled.attachments_policy, "always");
    assert_eq!(
        unbundled.estimated_input_tokens,
        tokenizer::count_chat(&[("user", "")]).unwrap()
    );
    assert!(unbundled.bundled.is_none());
    assert_eq!(unbundled.attachments.len(), 1);
    assert_eq!(unbundled.attachments[0].path, path);
    assert!(!unbundled.attachments[0].generated_bundle);

    let mut bundled = BrowserPrompt::remote_fallback(
        &files,
        composer,
        Some(PendingBundle {
            format: BundleFormat::Zip,
            scope: BundleScope::All,
        }),
        root.path(),
    )
    .unwrap();
    assert_eq!(
        bundled.composer_text,
        "The attached `attachments-bundle.zip` contains 1 selected file with relative paths preserved. Extract it into a temporary directory, then inspect the resulting file tree with filesystem and search tools before answering."
    );
    let metadata = bundled.bundled.as_ref().unwrap();
    assert_eq!(metadata.format, BundleFormat::Zip);
    assert_eq!(metadata.original_count, 1);
    assert_eq!(bundled.attachments.len(), 1);
    assert!(bundled.attachments[0].generated_bundle);
    assert_eq!(bundled.attachments[0].path, metadata.bundle_path);

    let bundle_dir = metadata.bundle_path.parent().unwrap().to_path_buf();
    {
        let mut archive =
            zip::ZipArchive::new(fs::File::open(&metadata.bundle_path).unwrap()).unwrap();
        assert_eq!(archive.len(), 1);
        let mut archived_bytes = Vec::new();
        archive
            .by_name(display_path)
            .unwrap()
            .read_to_end(&mut archived_bytes)
            .unwrap();
        assert_eq!(archived_bytes.as_slice(), bytes);
    }

    bundled.cleanup().unwrap();
    assert!(!bundle_dir.exists());
    assert_eq!(fs::read(path).unwrap().as_slice(), bytes);
}

#[test]
fn preview_counts_only_nonblank_string_followups_and_reports_cookie_sources() {
    let root = tempfile::tempdir().unwrap();
    let mut plan =
        BrowserPrompt::assemble(&[], "PUBLIC-question", None, &Value::Null, root.path()).unwrap();
    plan.estimated_input_tokens = 1_234_567;
    let followups = [
        json!(" PUBLIC-first question "),
        Value::Null,
        json!(" \t\n "),
        json!(42),
        json!("PUBLIC-second question"),
    ];

    for (config, target, cookies) in [
        (
            json!({
                "modelStrategy": "current",
                "inlineCookies": [{"name": "PUBLIC-cookie", "value": "PUBLIC-cookie-value"}],
                "cookieSync": true
            }),
            "picker=current; requested=PUBLIC-model",
            "inline payload (1) via inline.",
        ),
        (
            json!({
                "modelStrategy": "ignore",
                "inlineCookies": [
                    {"name": "PUBLIC-cookie", "value": "PUBLIC-cookie-value"},
                    {"name": "PUBLIC-other", "value": "PUBLIC-cookie-value-two"}
                ],
                "inlineCookiesSource": "PUBLIC-fixture"
            }),
            "picker=ignore; requested=PUBLIC-model",
            "inline payload (2) via PUBLIC-fixture.",
        ),
        (
            json!({
                "desiredModel": " \t ",
                "cookieSync": true,
                "cookieNames": ["PUBLIC-first", 42, "PUBLIC-second"]
            }),
            "PUBLIC-model",
            "copy from Chrome (PUBLIC-first, PUBLIC-second).",
        ),
        (
            json!({"cookieSync": true, "cookieNames": []}),
            "PUBLIC-model",
            "copy from Chrome (all from Chrome profile).",
        ),
    ] {
        let lines = plan
            .preview_lines(&config, "PUBLIC-model", "preview", &followups)
            .unwrap();
        assert_eq!(
            lines[0],
            format!(
                "[preview] Alcuin ({}) browser mode ({target}) with ~1,234,567 tokens.",
                env!("CARGO_PKG_VERSION")
            )
        );
        let expected_cookies = format!("[preview] Cookies: {cookies}");
        assert!(lines.contains(&expected_cookies), "{lines:?}");
        assert!(
            lines
                .iter()
                .all(|line| !line.contains("PUBLIC-cookie-value"))
        );

        let followup_index = lines
            .iter()
            .position(|line| line == "[preview] Browser follow-ups: 2 additional prompt(s).")
            .expect("preview must count both nonblank strings and ignore other values");
        assert_eq!(
            lines[followup_index + 1],
            "[preview] Multi-turn is explicit only: Alcuin will send these prompts in order, but it never invents follow-ups automatically."
        );
    }

    let deep = json!({"researchMode": "deep"});
    let error = plan
        .preview_lines(&deep, "PUBLIC-model", "preview", &followups)
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Browser follow-ups are not supported with Deep Research mode. Put the full research plan into the initial prompt or run a normal browser consult for multi-turn review."
    );
    let lines = plan
        .preview_lines(&deep, "PUBLIC-model", "preview", &followups[1..4])
        .unwrap();
    assert!(
        lines
            .iter()
            .all(|line| !line.contains("Browser follow-ups:"))
    );
}

#[test]
fn full_preview_renders_empty_and_nonempty_composers_and_distinguishes_inline_and_bundled_files() {
    let root = tempfile::tempdir().unwrap();
    let mut plan = BrowserPrompt::assemble(&[], "", None, &Value::Null, root.path()).unwrap();
    let lines = plan
        .preview_lines(&Value::Null, "PUBLIC-model", "full", &[])
        .unwrap();
    assert_eq!(
        &lines[lines.len() - 3..],
        &["", "Composer Text", "(empty prompt)"]
    );

    plan.composer_text = "PUBLIC-inline fixture".into();
    plan.token_estimate_includes_inline_files = true;
    for (count, noun) in [(1, "file"), (2, "files")] {
        plan.inline_file_count = count;
        let lines = plan
            .preview_lines(&Value::Null, "PUBLIC-model", "full", &[])
            .unwrap();
        assert!(lines[0].ends_with(&format!("tokens (includes {count} inline {noun}).")));
        let inline_index = lines
            .iter()
            .position(|line| line == "[preview] Inline file content:")
            .unwrap();
        assert_eq!(
            lines[inline_index + 1],
            format!("  • {count} {noun} pasted directly into the composer.")
        );
        assert_eq!(
            &lines[lines.len() - 3..],
            &["", "Composer Text", "PUBLIC-inline fixture"]
        );
    }

    // Preview uses attachment metadata; it does not need large files on disk.
    plan.composer_text = "PUBLIC-bundled fixture".into();
    plan.inline_file_count = 0;
    plan.token_estimate_includes_inline_files = false;
    let sizes = [
        ("PUBLIC-bundle.zip", 1023, "1023 B"),
        ("PUBLIC-kib.png", 1024, "1.0 KB"),
        ("PUBLIC-mib.png", 1024 * 1024, "1.0 MB"),
    ];
    plan.attachments = sizes
        .iter()
        .enumerate()
        .map(|(index, &(name, size_bytes, _))| Attachment {
            path: root.path().join(name),
            display_path: name.into(),
            size_bytes,
            generated_bundle: index == 0,
            scanned_sha256: None,
        })
        .collect();
    let bundle_path = plan.attachments[0].path.clone();
    plan.bundled = Some(BundleMetadata {
        original_count: 4,
        bundle_path: bundle_path.clone(),
        format: BundleFormat::Zip,
    });

    let lines = plan
        .preview_lines(&Value::Null, "PUBLIC-model", "full", &[])
        .unwrap();
    let bundle_index = lines
        .iter()
        .position(|line| line == "[preview] Bundled upload:")
        .unwrap();
    for (offset, (name, _, size)) in sizes.into_iter().enumerate() {
        assert_eq!(
            lines[bundle_index + 1 + offset],
            format!("  • {name} ({size})")
        );
    }
    assert_eq!(
        lines[bundle_index + 1 + sizes.len()],
        format!("  (bundled 4 files into {})", bundle_path.display())
    );
    assert_eq!(
        &lines[lines.len() - 3..],
        &["", "Composer Text", "PUBLIC-bundled fixture"]
    );
}
