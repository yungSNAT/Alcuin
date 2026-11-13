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

/// Every command path in the captured command specs, such as `["project-sources", "list"]`.
fn command_paths() -> Vec<Vec<String>> {
    fn walk(spec: &serde_json::Value, parent: &[String], paths: &mut Vec<Vec<String>>) {
        let name = spec["command"].as_str().unwrap().split_whitespace().next();
        let mut path = parent.to_vec();
        path.push(name.unwrap().to_owned());
        for child in spec["children"].as_array().into_iter().flatten() {
            walk(child, &path, paths);
        }
        paths.push(path);
    }
    let specs: serde_json::Value =
        serde_json::from_str(include_str!("../src/cli_commands.json")).unwrap();
    let mut paths = vec![];
    for spec in specs.as_array().unwrap() {
        walk(spec, &[], &mut paths);
    }
    paths
}

/// Whether `help` names `flag` whole, not as the start of a longer option.
fn names(help: &str, flag: &str) -> bool {
    help.match_indices(flag).any(|(at, _)| {
        !help[at + flag.len()..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '-')
    })
}

#[test]
fn subcommand_help_omits_refused_options() {
    let home = tempfile::tempdir().unwrap();
    let paths = command_paths();
    assert!(paths.len() > 20, "{paths:?}");
    for path in paths {
        let mut args: Vec<&str> = path.iter().map(String::as_str).collect();
        args.push("--help");
        let output = cli(home.path(), &args);
        assert!(output.status.success(), "{args:?}");
        let help = String::from_utf8(output.stdout).unwrap();
        for flag in REFUSED.iter().chain(&["--clear"]) {
            assert!(!names(&help, flag), "{args:?} lists {flag}:\n{help}");
        }
    }
    for args in [
        &["status", "--clear"][..],
        &["session", "--clear"],
        &["project-sources", "list", "--browser-reuse-wait", "30s"],
    ] {
        let output = cli(home.path(), args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        let flag = args.iter().find(|arg| arg.starts_with("--")).unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(&format!("{flag} {NOT_SUPPORTED}")),
            "{args:?}: {stderr}"
        );
    }
}

#[test]
fn mcp_tools_put_an_before_alcuin() {
    let tools = include_str!("../fixtures/mcp-tools.json");
    assert!(!tools.contains("a Alcuin"), "an MCP tool says 'a Alcuin'");
}
