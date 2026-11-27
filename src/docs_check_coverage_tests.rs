use super::*;

#[test]
fn slash_flags_stops_at_slash_not_followed_by_flag_segment() {
    let cases: [(&str, &[&str]); 4] = [
        // `/x` after a valid `/-baz` addition breaks the loop, keeping earlier additions.
        ("--foo-bar/-baz/x", &["--foo-bar", "--foo-baz"]),
        // A slash followed by a plain word breaks immediately; no additions, so nothing recorded.
        ("--foo/bar", &[]),
        // A slash at end of line also breaks.
        ("use --foo-bar/ here", &[]),
        // A slash followed by an uppercase dash segment is not a flag segment.
        ("--foo-bar/-Baz", &[]),
    ];
    for (line, expected) in cases {
        let got = slash_flags(line);
        let expected: BTreeSet<String> = expected.iter().map(|s| s.to_string()).collect();
        assert_eq!(got, expected, "line: {line}");
    }
}

#[test]
fn format_result_omits_scope_parentheses_when_issue_has_no_section_or_command() {
    let result = DocsCheck {
        checked_files: vec!["README.md".into(), "docs/spec.md".into()],
        checked_flags: vec!["--public-missing".into(), "--public-other".into()],
        issues: vec![
            Issue {
                file: "README.md".into(),
                flag: "--public-missing".into(),
                section: None,
                command: None,
            },
            Issue {
                file: "docs/spec.md".into(),
                flag: "--public-other".into(),
                section: Some("Usage".into()),
                command: Some("alcuin serve".into()),
            },
            Issue {
                file: "docs/spec.md".into(),
                flag: "--public-other".into(),
                section: None,
                command: Some("alcuin".into()),
            },
        ],
    };
    let text = format_result(&result);
    assert_eq!(
        text,
        "Docs/help drift:\n\
         - README.md mentions --public-missing, but CLI help does not expose --public-missing\n\
         - docs/spec.md (Usage, alcuin serve) mentions --public-other, but CLI help does not expose --public-other\n\
         - docs/spec.md (alcuin) mentions --public-other, but CLI help does not expose --public-other"
    );
    let first = text.lines().nth(1).unwrap();
    assert!(!first.contains('('));
}
