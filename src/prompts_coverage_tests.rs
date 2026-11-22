use super::*;
#[cfg(unix)]
use std::io::ErrorKind;

#[test]
fn path_helpers_handle_nameless_paths_and_disjoint_components() {
    assert!(!sensitive_name(Path::new("")));
    assert!(sensitive_name(Path::new("PUBLIC-fixture.PEM")));
    assert_eq!(
        relative(Path::new(r"PUBLIC\notes.txt"), Path::new("workspace")),
        "PUBLIC/notes.txt"
    );
    assert_eq!(
        absolute(Path::new("."), "./drafts/../PUBLIC-notes.txt"),
        PathBuf::from("PUBLIC-notes.txt")
    );
}

#[cfg(unix)]
#[test]
fn selection_rejects_a_fifo_and_scanning_rejects_a_directory() {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let fifo = root.join("public.fifo");
    let name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: `name` is a live, NUL-terminated path inside the temporary directory.
    let status = unsafe { libc::mkfifo(name.as_ptr(), 0o600) };
    assert_eq!(
        status,
        0,
        "could not create fixture FIFO: {}",
        std::io::Error::last_os_error()
    );

    let error = resolve_files(&["public.fifo".into()], &root, 0, true).unwrap_err();
    assert_eq!(error.to_string(), "Not a file or directory: public.fifo");

    // Either the native open rejects the directory or the scanner must reject it.
    let expected = match crate::native_fs::NativeEntry::open(&root, false) {
        Ok(_) => format!("{} is not a regular file", root.display()),
        Err(error) => error.to_string(),
    };
    assert_eq!(scan_file(&root, 0, true).unwrap_err().to_string(), expected);
}

#[cfg(unix)]
#[test]
fn confinement_defers_missing_inputs_but_reports_non_directory_components() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let file = root.join("public.txt");
    fs::write(&file, "PUBLIC-fixture").unwrap();

    let missing = ["missing.txt".into()];
    assert!(confine_inputs(&missing, root, root).is_ok());
    assert_eq!(
        resolve_files(&missing, root, 0, true)
            .unwrap_err()
            .to_string(),
        "Missing file or directory: missing.txt"
    );

    let error = confine_inputs(&["public.txt/child.txt".into()], root, root).unwrap_err();
    assert_eq!(error.to_string(), "Cannot resolve public.txt/child.txt");
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        ErrorKind::NotADirectory
    );

    let Err(error) = FileWalker::rules_at(&file) else {
        panic!("looking up ignore rules beneath a regular file must fail");
    };
    assert_eq!(
        error.to_string(),
        format!("Cannot read {}", file.join(".gitignore").display())
    );
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        ErrorKind::NotADirectory
    );
}

#[cfg(unix)]
#[test]
fn directory_and_glob_walks_propagate_ignore_rule_errors() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir(root.join(".git")).unwrap();
    let rules = root.join("source/.gitignore");
    fs::create_dir_all(&rules).unwrap();
    fs::write(root.join("source/public.txt"), "PUBLIC-fixture").unwrap();

    let expected = format!("Cannot apply {}: it is not a regular file", rules.display());
    for selection in ["source", "source/*.txt"] {
        let error = resolve_files(&[selection.into()], root, 0, true).unwrap_err();
        assert_eq!(error.to_string(), expected, "selection: {selection}");
    }
}

#[test]
fn ignore_rule_compilation_failure_names_the_rule_file() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let rules = root.join(".gitignore");
    // The glob parses, but its nested alternatives exceed the regex parser's nesting limit.
    let pattern = format!(
        "/{}PUBLIC-leaf{}\n",
        "{PUBLIC-branch,".repeat(300),
        "}".repeat(300)
    );
    fs::write(&rules, pattern).unwrap();

    let mut builder = ignore::gitignore::GitignoreBuilder::new(root);
    assert!(builder.add(&rules).is_none(), "the glob must parse");

    let Err(error) = FileWalker::rules_at(root) else {
        panic!("an ignore rule that cannot compile must fail the lookup");
    };
    assert_eq!(
        error.to_string(),
        format!("Cannot apply {}", rules.display())
    );
    assert!(error.downcast_ref::<ignore::Error>().is_some());
}

#[test]
fn literal_ignore_lookup_does_not_ignore_the_workspace_or_a_path_without_a_rule_root() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("workspace");
    fs::create_dir(&cwd).unwrap();
    fs::write(cwd.join(".gitignore"), "*\n").unwrap();
    let walker = FileWalker {
        cwd: &cwd,
        literal: &[],
        directories: &[],
        excludes: &[],
        prunes: &[],
    };

    assert!(!walker.literal_ignored(Path::new("/")).unwrap());
    assert!(!walker.literal_ignored(&cwd).unwrap());
    assert!(walker.literal_ignored(&cwd.join("public.txt")).unwrap());
}

#[test]
fn expanded_selection_filters_literal_files_but_bare_exclusion_keeps_literal_order() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir(root.join(".git")).unwrap();
    fs::create_dir(root.join("blocked")).unwrap();
    fs::write(root.join(".gitignore"), "blocked/\n").unwrap();
    for name in ["visible.txt", "blocked/child.txt", "excluded.txt"] {
        fs::write(root.join(name), "PUBLIC-fixture").unwrap();
    }

    let display_paths = |inputs: &[&str]| {
        let inputs = inputs
            .iter()
            .copied()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        resolve_files(&inputs, root, 0, true)
            .unwrap()
            .into_iter()
            .map(|file| file.display_path)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        display_paths(&["visible.txt", " ! ", "blocked/child.txt"]),
        ["visible.txt", "blocked/child.txt"]
    );
    assert_eq!(
        display_paths(&[
            "blocked/child.txt",
            "excluded.txt",
            "visible.txt",
            "!excluded.txt",
            "missing/*.txt",
        ]),
        ["visible.txt"]
    );
}

#[test]
fn final_selection_rechecks_default_ignores_after_the_walk() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir(root.join(".git")).unwrap();
    let cache = root.join("node_modules");
    let later = root.join("z-later");
    fs::create_dir(&cache).unwrap();
    fs::create_dir(&later).unwrap();
    let rules = cache.join(".gitignore");
    fs::write(&rules, "# PUBLIC-fixture\n").unwrap();
    fs::write(cache.join("stale.txt"), "PUBLIC-stale").unwrap();
    fs::write(root.join("visible.txt"), "PUBLIC-visible").unwrap();

    let mut visited = Vec::new();
    let files = resolve_files_with(&["**/*.txt".into()], root, 0, true, &[], &mut |path| {
        visited.push(path.to_owned());
        if path == later {
            fs::remove_file(&rules).unwrap();
        }
    })
    .unwrap();

    assert_eq!(visited, [root.to_owned(), cache, later]);
    assert!(!rules.exists());
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].display_path, "visible.txt");
    assert_eq!(files[0].content, "PUBLIC-visible");
}

#[test]
fn parser_skips_invalid_indices_and_line_counts_then_recovers() {
    let following = "### File 7: kept.txt\nLines: 1-1\n```\n1 | PUBLIC-kept\n```";
    for malformed in [
        "### File x: bad.txt\n```\nPUBLIC-rejected\n```",
        "### File : bad.txt\n```\nPUBLIC-rejected\n```",
        "### File: bad.txt\nLines: 1-2\n```\n1 | PUBLIC-rejected\n```",
        "### File: bad.txt\nLines: 0\n```\n1 | PUBLIC-rejected\n```",
    ] {
        let text = format!("{malformed}\n\n{following}");
        assert_eq!(
            parse_file_sections(&text),
            [("kept.txt".to_owned(), "PUBLIC-kept".to_owned())],
            "malformed section: {malformed}"
        );
    }
}

#[test]
fn byte_formatting_switches_units_at_binary_thresholds() {
    for (bytes, expected) in [
        (1023, "1023 B"),
        (1024, "1 KB"),
        (1536, "1.5 KB"),
        (1023 * 1024, "1023 KB"),
        (1024 * 1024, "1 MB"),
    ] {
        assert_eq!(format_bytes(bytes), expected, "input: {bytes} bytes");
    }
}
