use alcuin::docs_check;
use serde_json::Value;
use std::{fs, process::Command};
#[test]
fn flags_are_complete_and_slash_shorthand_does_not_create_prefix_fragments() {
    assert_eq!(
        docs_check::extract_flags(
            "Use `--no-azure`, `--http-timeout <ms>`, --browser-inline-cookies[(-file)], --remote-host/--remote-token, --browser-auto-reattach-delay/-interval/-timeout, and --browser-auto-reattach-*"
        ),
        vec![
            "--browser-auto-reattach-delay",
            "--browser-auto-reattach-interval",
            "--browser-auto-reattach-timeout",
            "--browser-inline-cookies",
            "--http-timeout",
            "--no-azure",
            "--remote-host",
            "--remote-token"
        ]
    );
    assert_eq!(
        docs_check::extract_flags(
            "corepack pnpm@11.28.0 install --frozen-lockfile\nnpm install --ignore-scripts\npnpm run check --silent\ncargo install --locked --path .\nalcuin --stale-flag\nDocumented: `--known`."
        ),
        vec!["--known", "--stale-flag"]
    );
}
#[test]
fn inherited_command_aliases_and_core_sections_keep_their_own_flag_scope() {
    let home = tempfile::tempdir().unwrap();
    fs::write(home.path().join("flags.md"),"alcuin status --json\noracle status --json\nnpx @steipete/oracle status --json\npnpm exec alcuin status --json\ncorepack pnpm@11.28.0 exec alcuin status --json\n## Core consult flags\n`--json`\n").unwrap();
    let result = docs_check::check(home.path(), &["flags.md".into()]).unwrap();
    assert_eq!(result.issues.len(), 6);
    assert_eq!(
        result
            .issues
            .iter()
            .filter(|i| i.command.as_deref() == Some("alcuin status"))
            .count(),
        5
    );
    assert_eq!(
        result
            .issues
            .iter()
            .filter(|i| i.section.as_deref() == Some("Core consult flags"))
            .count(),
        1
    );
    assert_eq!(result.checked_flags, vec!["--json"]);
    assert!(docs_check::format_result(&result).contains("Docs/help drift:"));
}
#[test]
fn other_tools_command_lines_keep_their_own_flags() {
    let home = tempfile::tempdir().unwrap();
    fs::write(
        home.path().join("install.md"),
        "npx skills add yungSNAT/Alcuin --skill alcuin --global --yes\n$ gh attestation verify a.zip --repo yungSNAT/Alcuin\ncodex mcp add alcuin --env ORACLE_ENGINE=browser -- alcuin-mcp\nIn Codex, `codex mcp add alcuin --env ORACLE_ENGINE=browser -- alcuin-mcp` adds it.\nnpx @steipete/oracle status --json\n",
    )
    .unwrap();
    let result = docs_check::check(home.path(), &["install.md".into()]).unwrap();
    let issues: Vec<_> = result
        .issues
        .iter()
        .map(|issue| (issue.flag.as_str(), issue.command.as_deref()))
        .collect();
    // Only the line that runs Oracle, under `npx`, is read for Alcuin's flags.
    assert_eq!(issues, [("--json", Some("alcuin status"))]);
}
#[test]
fn explicit_missing_paths_refuse_and_default_paths_do_not_visit_unrelated_documents() {
    let home = tempfile::tempdir().unwrap();
    assert!(docs_check::check(home.path(), &[]).is_err());
    assert_eq!(
        docs_check::check(home.path(), &["missing.md".into()])
            .unwrap_err()
            .to_string(),
        "Docs check path not found: missing.md"
    );
    fs::write(
        home.path().join("README.md"),
        "Use `--engine` and `--file`.\n",
    )
    .unwrap();
    fs::write(home.path().join("unrelated.md"), "`--stale-flag`\n").unwrap();
    let result = docs_check::check(home.path(), &[]).unwrap();
    assert!(result.issues.is_empty());
    assert_eq!(result.checked_files, vec!["README.md"]);
    assert_eq!(
        docs_check::format_result(&result),
        "Docs/help check: ok (2 flags, 1 files)"
    );
    // A file named `docs` leaves the default check on the README.
    fs::write(home.path().join("docs"), "`--stale-flag`\n").unwrap();
    let result = docs_check::check(home.path(), &[]).unwrap();
    assert_eq!(result.checked_files, vec!["README.md"]);
    fs::remove_file(home.path().join("docs")).unwrap();
    fs::create_dir(home.path().join("docs")).unwrap();
    fs::write(
        home.path().join("docs/new-guide.md"),
        "alcuin --stale-guide-flag\n",
    )
    .unwrap();
    fs::write(home.path().join("docs/notes.txt"), "`--ignored`\n").unwrap();
    let result = docs_check::check(home.path(), &[]).unwrap();
    assert_eq!(result.checked_files, vec!["README.md", "docs/new-guide.md"]);
    assert_eq!(result.issues.len(), 1);
    assert_eq!(result.issues[0].file, "docs/new-guide.md");
    assert_eq!(result.issues[0].flag, "--stale-guide-flag");
}
#[test]
fn default_check_covers_readme_and_all_repository_guides() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let result = docs_check::check(root, &[]).unwrap();
    assert!(
        result.issues.is_empty(),
        "{}",
        docs_check::format_result(&result)
    );
    let mut guides: Vec<String> = fs::read_dir(root.join("docs"))
        .unwrap()
        .map(|entry| format!("docs/{}", entry.unwrap().file_name().to_str().unwrap()))
        .filter(|path| path.ends_with(".md"))
        .collect();
    guides.sort();
    let mut checked_guides: Vec<String> = result
        .checked_files
        .iter()
        .filter(|path| path.starts_with("docs/"))
        .cloned()
        .collect();
    checked_guides.sort();
    assert_eq!(checked_guides, guides);
    assert!(result.checked_files.contains(&"README.md".to_owned()));
}
#[test]
fn actual_cli_reports_drift_as_exit_one_and_json_stays_parseable() {
    let home = tempfile::tempdir().unwrap();
    let run = |file: &str| {
        Command::new(env!("CARGO_BIN_EXE_alcuin"))
            .env_clear()
            .envs(std::env::var_os("LLVM_PROFILE_FILE").map(|value| ("LLVM_PROFILE_FILE", value)))
            .env("ORACLE_DISABLE_NOTIFICATIONS", "1")
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("ORACLE_HOME_DIR", home.path())
            .current_dir(home.path())
            .args(["docs", "check", "--docs-path", file, "--json"])
            .output()
            .unwrap()
    };
    fs::write(home.path().join("good.md"), "alcuin doctor --json\n").unwrap();
    let output = run("good.md");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["issues"], serde_json::json!([]));
    fs::write(home.path().join("bad.md"), "alcuin status --json\n").unwrap();
    let output = run("bad.md");
    assert_eq!(output.status.code(), Some(1));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["issues"][0]["command"], "alcuin status");
    let output = run("missing.md");
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Docs check path not found: missing.md")
    );
}
#[test]
fn issues_sort_files_and_flags_like_locale_compare() {
    let home = tempfile::tempdir().unwrap();
    fs::create_dir(home.path().join("docs")).unwrap();
    for name in ["README.md", "docs/a.md"] {
        fs::write(
            home.path().join(name),
            "alcuin --public-zeta-stale --public-alpha-stale\n",
        )
        .unwrap();
    }
    let result = docs_check::check(home.path(), &["README.md".into(), "docs/a.md".into()]).unwrap();
    let order: Vec<_> = result
        .issues
        .iter()
        .map(|issue| format!("{} {}", issue.file, issue.flag))
        .collect();
    assert_eq!(
        order,
        [
            "docs/a.md --public-alpha-stale",
            "docs/a.md --public-zeta-stale",
            "README.md --public-alpha-stale",
            "README.md --public-zeta-stale"
        ]
    );
}
#[test]
fn readme_documents_ground_within_its_declared_flags() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let readme = fs::read_to_string(root.join("README.md")).unwrap();
    assert!(readme.contains("`alcuin ground <id> --json`"));
    let result = docs_check::check(root, &["README.md".into()]).unwrap();
    let ground: Vec<_> = result
        .issues
        .iter()
        .filter(|issue| issue.command.as_deref() == Some("alcuin ground"))
        .collect();
    assert!(ground.is_empty(), "{ground:?}");
    // `--json` is declared on `ground`, so the check would catch a flag it lacks.
    let home = tempfile::tempdir().unwrap();
    fs::write(
        home.path().join("ground.md"),
        "alcuin ground <id> --json --hours 2\n",
    )
    .unwrap();
    let result = docs_check::check(home.path(), &["ground.md".into()]).unwrap();
    let flagged: Vec<_> = result
        .issues
        .iter()
        .map(|issue| issue.flag.as_str())
        .collect();
    assert_eq!(flagged, ["--hours"]);
}
#[test]
fn guides_name_only_real_commands_and_flags() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    // Every guide, so a new one is checked without being listed here.
    let mut paths: Vec<String> = fs::read_dir(root.join("docs"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".md"))
        .map(|name| format!("docs/{name}"))
        .collect();
    paths.sort();
    assert!(
        paths.contains(&"docs/quickstart.md".to_owned()),
        "{paths:?}"
    );
    // Flags, against the captured definitions the help renders from.
    let result = docs_check::check(root, &paths).unwrap();
    assert!(
        result.issues.is_empty(),
        "{}",
        docs_check::format_result(&result)
    );
    // Commands: `alcuin <command>`, and `<subcommand>` too for a command that has them.
    let commands: Value =
        serde_json::from_str(&fs::read_to_string(root.join("src/cli_commands.json")).unwrap())
            .unwrap();
    let name = |spec: &Value| {
        spec["command"]
            .as_str()
            .unwrap()
            .split(' ')
            .next()
            .unwrap()
            .to_owned()
    };
    let known: std::collections::BTreeMap<String, Vec<String>> = commands
        .as_array()
        .unwrap()
        .iter()
        .map(|spec| {
            (
                name(spec),
                spec["children"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(name)
                    .collect(),
            )
        })
        .collect();
    let word = |word: &&str| {
        word.starts_with(|c: char| c.is_ascii_lowercase())
            && word.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
    };
    let mut unknown = vec![];
    for path in &paths {
        for line in fs::read_to_string(root.join(path)).unwrap().lines() {
            let Some(rest) = line.trim().strip_prefix("alcuin ") else {
                continue;
            };
            let mut words = rest.split_whitespace().take_while(word);
            let Some(command) = words.next() else {
                continue;
            };
            let named = known.get(command).is_some_and(|children| {
                children.is_empty()
                    || words
                        .next()
                        .is_none_or(|child| children.iter().any(|c| c == child))
            });
            if !named {
                unknown.push(format!("{path}: {line}"));
            }
        }
    }
    assert!(unknown.is_empty(), "{unknown:#?}");
}
/// Long options on the option lines of `--help` output, which wraps descriptions onto their
/// own lines.
fn help_flags(help: &str) -> Vec<String> {
    let mut flags = vec![];
    for line in help.lines() {
        // Option lines are indented 2 spaces; wrapped descriptions further, and may start with
        // an option they mention.
        let Some(line) = line.strip_prefix("  ").filter(|line| line.starts_with('-')) else {
            continue;
        };
        let names = line.split("  ").next().unwrap_or("");
        for word in names.split([' ', ',']) {
            let flag = word.split(['<', '[', '=']).next().unwrap_or("");
            if flag.starts_with("--") && flag.len() > 2 {
                flags.push(flag.to_owned());
            }
        }
    }
    flags
}
#[test]
fn cli_reference_covers_every_command_and_option_its_help_shows() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let reference = fs::read_to_string(root.join("docs/cli-reference.md")).unwrap();
    let home = tempfile::tempdir().unwrap();
    let help = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_alcuin"))
            .env_clear()
            .envs(std::env::var_os("LLVM_PROFILE_FILE").map(|value| ("LLVM_PROFILE_FILE", value)))
            .env("ORACLE_DISABLE_NOTIFICATIONS", "1")
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("ORACLE_HOME_DIR", home.path())
            .args(args)
            .arg("--help")
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}");
        String::from_utf8(output.stdout).unwrap()
    };
    let mut paths = vec![vec![]];
    let commands: Value =
        serde_json::from_str(&fs::read_to_string(root.join("src/cli_commands.json")).unwrap())
            .unwrap();
    let name = |spec: &Value| {
        spec["command"]
            .as_str()
            .unwrap()
            .split(' ')
            .next()
            .unwrap()
            .to_owned()
    };
    for spec in commands.as_array().unwrap() {
        paths.push(vec![name(spec)]);
        for child in spec["children"].as_array().unwrap() {
            paths.push(vec![name(spec), name(child)]);
        }
    }
    let mut missing = vec![];
    for path in &paths {
        if !path.is_empty() && !reference.contains(&format!("alcuin {}", path.join(" "))) {
            missing.push(format!("command alcuin {}", path.join(" ")));
        }
        let args: Vec<&str> = path.iter().map(String::as_str).collect();
        let flags = help_flags(&help(&args));
        assert!(!flags.is_empty(), "{path:?}");
        for flag in flags {
            if !reference.contains(&format!("`{flag}`"))
                && !reference.contains(&format!("`{flag} "))
            {
                missing.push(format!("{} {flag}", path.join(" ")));
            }
        }
    }
    assert!(missing.is_empty(), "{missing:#?}");
}
#[test]
fn mcp_tool_reference_covers_every_tool_input_and_output() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    // A Windows checkout may have CRLF line ends; the sections are found by their headings.
    let reference = fs::read_to_string(root.join("docs/mcp-tools.md"))
        .unwrap()
        .replace("\r\n", "\n");
    // The fixture is what the server's tools/list returns.
    let tools: Value =
        serde_json::from_str(&fs::read_to_string(root.join("fixtures/mcp-tools.json")).unwrap())
            .unwrap();
    let mut missing = vec![];
    for tool in tools["tools"].as_array().unwrap() {
        let name = tool["name"].as_str().unwrap();
        // Each tool's own section, from its heading to the next.
        let Some((_, section)) = reference.split_once(&format!("\n## {name}\n")) else {
            missing.push(format!("tool {name}"));
            continue;
        };
        let section = section.split("\n## ").next().unwrap();
        for schema in ["inputSchema", "outputSchema"] {
            let Some(fields) = tool[schema]["properties"].as_object() else {
                continue;
            };
            for field in fields.keys() {
                if !section.contains(&format!("`{field}`")) {
                    missing.push(format!("{name} {schema} {field}"));
                }
            }
        }
    }
    assert!(missing.is_empty(), "{missing:#?}");
}
#[test]
fn command_substitutions_and_git_lines_keep_their_own_flags() {
    let home = tempfile::tempdir().unwrap();
    fs::write(
        home.path().join("files.md"),
        concat!(
            "alcuin -p \"Review\" --file $(git diff --name-only main...)\n",
            "In PowerShell, write `--file (git diff --diff-filter=d main...)`.\n",
            "git diff --name-only main... > changed.txt\n",
            "alcuin --stale-flag --file \"${files[@]}\"\n",
        ),
    )
    .unwrap();
    let result = docs_check::check(home.path(), &["files.md".into()]).unwrap();
    let flags: Vec<_> = result
        .issues
        .iter()
        .map(|issue| issue.flag.as_str())
        .collect();
    // git's options, run alone or inside $(...) or PowerShell's (...), are not Alcuin's.
    assert_eq!(flags, ["--stale-flag"]);
}
