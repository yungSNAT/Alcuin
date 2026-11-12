//! `--help` lists only what the CLI accepts; options it refuses stay parsed for old scripts.
use std::{path::Path, process::Command};

/// Options this build refuses. Help omits them; parsing keeps them, so a saved script gets the
/// refusal rather than an unknown-option error.
const REFUSED: [&str; 9] = [
    "--verbose-render",
    "--browser-capture-provider-native",
    "--retain-hours",
    "--browser-recheck-delay",
    "--browser-recheck-timeout",
    "--browser-reuse-wait",
    "--browser-auto-reattach-delay",
    "--browser-auto-reattach-interval",
    "--browser-auto-reattach-timeout",
];
const NOT_SUPPORTED: &str = "is not supported in this build";

fn cli(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_alcuin"))
        .env_clear()
        .env("ORACLE_DISABLE_NOTIFICATIONS", "1")
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("ORACLE_HOME_DIR", home)
        .current_dir(home)
        .args(args)
        .output()
        .unwrap()
}

/// Long options in `alcuin --help --verbose`, each with a sample value when it takes one.
fn listed_options(home: &Path) -> Vec<(String, Option<&'static str>)> {
    let output = cli(home, &["--help", "--verbose"]);
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    let mut options = vec![];
    for line in help.lines() {
        if !line.starts_with("  -") {
            continue;
        }
        let mut words = line.split_whitespace();
        let Some(flag) = words.find(|word| word.starts_with("--")) else {
            continue;
        };
        let value = match words.next() {
            Some(placeholder) if placeholder.starts_with('<') => {
                Some(if placeholder.contains("ms|") {
                    "30s"
                } else {
                    "1"
                })
            }
            _ => None,
        };
        options.push((flag.trim_end_matches(',').to_owned(), value));
    }
    assert!(options.len() > 50, "{help}");
    options
}

#[test]
fn every_option_in_help_is_accepted() {
    let home = tempfile::tempdir().unwrap();
    for (flag, value) in listed_options(home.path()) {
        // `--browser-approval-wait` is read by `alcuin serve`, whose help omits root options.
        if matches!(
            flag.as_str(),
            "--help" | "--version" | "--debug-help" | "--browser-approval-wait"
        ) {
            continue;
        }
        let mut args = vec![flag.as_str()];
        args.extend(value);
        args.push("status");
        let output = cli(home.path(), &args);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains(NOT_SUPPORTED), "{args:?}: {stderr}");
    }
}

#[test]
fn refused_options_are_hidden_from_help_but_still_parsed() {
    let home = tempfile::tempdir().unwrap();
    let listed: Vec<_> = listed_options(home.path())
        .into_iter()
        .map(|(flag, _)| flag)
        .collect();
    for flag in REFUSED {
        assert!(!listed.iter().any(|listed| listed == flag), "{flag}");
        let mut args = vec![flag];
        match flag {
            "--verbose-render" | "--browser-capture-provider-native" => {}
            "--retain-hours" => args.push("1"),
            _ => args.push("30s"),
        }
        args.push("status");
        let output = cli(home.path(), &args);
        assert_eq!(output.status.code(), Some(1), "{flag}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(&format!("{flag} {NOT_SUPPORTED}")),
            "{flag}: {stderr}"
        );
    }
}

#[test]
fn mcp_tools_put_an_before_alcuin() {
    let tools = include_str!("../fixtures/mcp-tools.json");
    assert!(!tools.contains("a Alcuin"), "an MCP tool says 'a Alcuin'");
}
