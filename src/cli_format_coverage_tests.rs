//! Coverage tests for the Commander-compatible CLI formatting helpers.

use super::*;
use clap::error::{ContextKind, ContextValue, ErrorKind};

fn argv(args: &[&str]) -> Vec<OsString> {
    args.iter().copied().map(OsString::from).collect()
}

#[test]
fn spec_options_lists_only_flagged_options() {
    let spec = serde_json::json!({
        "options": [
            {"flags": "-a, --alpha"},
            {"description": "PUBLIC-no-flags"},
            {"flags": "--beta <value>"}
        ]
    });
    let flags: Vec<&str> = spec_options(&spec).collect();
    assert_eq!(flags, ["-a, --alpha", "--beta <value>"]);

    let empty = serde_json::json!({"command": "PUBLIC-none"});
    assert_eq!(spec_options(&empty).count(), 0);
}

#[test]
fn command_path_stops_at_literal_and_follows_known_children() {
    assert!(command_path(&argv(&["alcuin", "--", "doctor"])).is_empty());
    assert_eq!(
        command_path(&argv(&["alcuin", "project-sources", "add", "PUBLIC-x"])),
        ["project-sources", "add"]
    );
    assert_eq!(
        command_path(&argv(&["alcuin", "project-sources", "PUBLIC-not-a-child"])),
        ["project-sources"]
    );
}

#[test]
fn negative_number_matches_commander_rules() {
    for (arg, expected) in [
        ("-5", true),
        ("5", false),
        ("-1.5", true),
        ("-.5", true),
        ("-1e5", true),
        ("-1e+5", true),
        ("-1e", false),
        ("-x", false),
        ("-", false),
    ] {
        assert_eq!(negative_number(arg), expected, "{arg}");
    }
}

#[test]
fn hoisting_handles_literal_in_short_group_and_session_path_with_value() {
    // A short group ending in '-' leaves a literal "--" that ends option parsing.
    assert_eq!(
        hoist_root_options(argv(&["alcuin", "doctor", "-v-", "PUBLIC-x"])),
        Ok(argv(&["alcuin", "--verbose", "doctor", "--", "PUBLIC-x"]))
    );
    // After session, --path=value belongs to the view and is not hoisted.
    assert_eq!(
        hoist_root_options(argv(&["alcuin", "session", "--path=PUBLIC-a"])),
        Ok(argv(&["alcuin", "session", "--path=PUBLIC-a"]))
    );
}

#[test]
fn candidates_skip_unknown_commands_and_add_help_for_known_ones() {
    let root_only = candidates(&[]);
    assert_eq!(candidates(&["PUBLIC-unknown".to_owned()]), root_only);
    let scoped = candidates(&["project-sources".to_owned()]);
    assert!(scoped.len() > root_only.len(), "{scoped:?}");
    assert!(scoped.iter().any(|flag| flag == "--help"));
}

#[test]
fn suggestions_without_dashes_list_ties_and_skip_single_characters() {
    let candidates = vec!["hat".to_owned(), "bat".into(), "x".into(), "bat".into()];
    assert_eq!(
        suggest_similar("cat", &candidates),
        "\n(Did you mean one of bat, hat?)"
    );
    assert_eq!(
        suggest_similar("cat", &["bat".to_owned()]),
        "\n(Did you mean bat?)"
    );
    assert_eq!(suggest_similar("c", &["x".to_owned()]), "");
}

#[test]
fn expected_arguments_defaults_to_one_without_a_spec() {
    assert_eq!(expected_arguments(&[]), 1);
    assert_eq!(expected_arguments(&["PUBLIC-unknown".to_owned()]), 1);
}

#[test]
fn commander_error_rewords_or_declines_clap_errors() {
    type Case<'a> = (
        ErrorKind,
        Vec<(ContextKind, ContextValue)>,
        &'a [&'a str],
        Option<&'a str>,
    );
    let cases: Vec<Case> = vec![
        (ErrorKind::DisplayHelp, vec![], &["alcuin"], None),
        (
            ErrorKind::UnknownArgument,
            vec![(ContextKind::InvalidArg, ContextValue::Bool(true))],
            &["alcuin"],
            None,
        ),
        (
            ErrorKind::InvalidValue,
            vec![(
                ContextKind::InvalidArg,
                ContextValue::String("--model <model>".into()),
            )],
            &["alcuin", "--model"],
            Some("error: option '-m, --model <model>' argument missing\n(use --help for usage)\n"),
        ),
        (
            ErrorKind::InvalidValue,
            vec![
                (
                    ContextKind::InvalidArg,
                    ContextValue::String("--model <model>".into()),
                ),
                (
                    ContextKind::InvalidValue,
                    ContextValue::String("PUBLIC-value".into()),
                ),
            ],
            &["alcuin", "--model", "PUBLIC-value"],
            None,
        ),
        (
            ErrorKind::MissingRequiredArgument,
            vec![(
                ContextKind::InvalidArg,
                ContextValue::Strings(vec!["--slug <slug>".into()]),
            )],
            &["alcuin"],
            None,
        ),
        (
            ErrorKind::MissingRequiredArgument,
            vec![(
                ContextKind::InvalidArg,
                ContextValue::Strings(vec!["[files...]".into()]),
            )],
            &["alcuin"],
            Some("error: missing required argument 'files'\n(use --help for usage)\n"),
        ),
        (
            ErrorKind::MissingRequiredArgument,
            vec![(
                ContextKind::InvalidArg,
                ContextValue::Strings(vec!["<prompt>".into()]),
            )],
            &["alcuin"],
            Some("error: missing required argument 'prompt'\n(use --help for usage)\n"),
        ),
    ];
    for (kind, contexts, args, expected) in cases {
        let mut error = clap::Error::new(kind);
        for (context, value) in contexts {
            error.insert(context, value);
        }
        assert_eq!(
            commander_error(&error, &argv(args)).as_deref(),
            expected,
            "{kind:?} {args:?}"
        );
    }
}

#[test]
fn azure_notes_describe_each_configuration() {
    assert_eq!(azure_note("auto", false, true), None);
    assert_eq!(
        azure_note("openai", true, false),
        Some("ignored, --provider openai/--no-azure is active")
    );
    assert_eq!(
        azure_note("auto", true, true),
        Some("active because Azure endpoint is configured")
    );
    assert_eq!(
        azure_note("auto", true, false),
        Some("configured, not used for this model")
    );
}

#[test]
fn project_sources_prints_dry_runs_and_completed_operations() {
    let dry_empty = serde_json::json!({
        "status": "dry-run",
        "operation": "add",
        "projectUrl": "https://example.invalid/PUBLIC-project"
    });
    assert_eq!(
        project_sources(&dry_empty),
        "Project Sources add dry run\nProject: https://example.invalid/PUBLIC-project"
    );

    let dry_plan = serde_json::json!({
        "status": "dry-run",
        "operation": "add",
        "projectUrl": "PUBLIC-p",
        "plannedUploads": [{"batch": 1, "displayPath": "PUBLIC-a.md"}]
    });
    assert_eq!(
        project_sources(&dry_plan),
        "Project Sources add dry run\nProject: PUBLIC-p\nPlanned uploads: 1\n  batch 1: PUBLIC-a.md"
    );

    let completed = serde_json::json!({
        "status": "completed",
        "operation": "sync",
        "projectUrl": "PUBLIC-p",
        "sourcesBefore": [{}, {}],
        "sourcesAfter": [{}, {}, {}],
        "added": [{"name": "PUBLIC-a.md"}, {"name": "PUBLIC-b.md"}]
    });
    assert_eq!(
        project_sources(&completed),
        "Project Sources sync completed\nProject: PUBLIC-p\nBefore: 2\nAfter: 3\nAdded: PUBLIC-a.md, PUBLIC-b.md"
    );

    let nothing_added = serde_json::json!({
        "operation": "remove",
        "projectUrl": "PUBLIC-p",
        "sourcesBefore": [{}],
        "added": []
    });
    assert_eq!(
        project_sources(&nothing_added),
        "Project Sources remove completed\nProject: PUBLIC-p\nBefore: 1\nAfter: 0"
    );
}
