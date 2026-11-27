use super::*;
use serde_json::json;

#[test]
fn read_diff_skips_no_newline_markers_and_ends_a_short_hunk_at_a_plain_line() {
    // A hunk with no `---`/`+++` headers: there is no section to record ranges in.
    let text = [
        "@@ -1,2 +1,2 @@",
        "-old line",
        "\\ No newline at end of file",
        "+new line",
        "plain trailing words",
        "",
    ]
    .join("\n");
    let (sections, sides) = read_diff(&text, "/work/repo").expect("text has a hunk");
    assert!(sections.is_empty());
    // The `\` marker is not content, and the plain line past the hunk ends it rather than
    // joining either side.
    assert_eq!(sides, vec!["new line".to_owned(), "old line".to_owned()]);

    // Without a section, a reference to the file cannot be grounded by the hunk.
    let diff = SentFile {
        path: "/work/frozen/headless.diff".into(),
        text,
    };
    let report = check("See src/lib.c:1.", &[diff], "/work/repo");
    assert_eq!(report.references.len(), 1);
    assert_eq!(report.references[0].status, Status::NotSent);

    // Text without any hunk is not read as a diff.
    assert!(read_diff("--- a/x\n+++ b/x\nno hunk here", "/work").is_none());
}

#[test]
fn current_files_skips_non_text_files_and_lists_changed_ones() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("bin.dat"),
        [0xffu8, 0xfe, 0x00, 0x80, 0x00, 0xc3],
    )
    .unwrap();
    fs::write(dir.path().join("notes.txt"), "PUBLIC-notes line\n").unwrap();
    let cwd = dir.path().to_str().unwrap();
    let meta = json!({
        "createdAt": "2000-01-01T00:00:00.000Z",
        "options": {
            "prompt": "PUBLIC-prompt text",
            "file": ["bin.dat", "notes.txt"]
        }
    });
    let (files, [changed, skipped]) =
        current_files(&meta, cwd, "session-1", None).expect("files resolve");

    assert_eq!(files[0].path, PROMPT);
    assert_eq!(files[0].text, "PUBLIC-prompt text");
    assert_eq!(files.len(), 2);
    assert!(files[1].path.ends_with("notes.txt"));
    assert_eq!(files[1].text, "PUBLIC-notes line\n");

    assert_eq!(skipped.len(), 1);
    assert!(skipped[0].ends_with("bin.dat"), "{skipped:?}");

    // Both files were written long after the session's creation time.
    assert_eq!(changed.len(), 2);
    assert!(changed.iter().any(|path| path.ends_with("bin.dat")));
    assert!(changed.iter().any(|path| path.ends_with("notes.txt")));

    let error = current_files(&json!({"options": {}}), cwd, "session-2", None)
        .err()
        .expect("no creation time");
    assert_eq!(
        error.to_string(),
        "Session \"session-2\" has no valid creation time."
    );
}
