#![cfg(all(unix, not(target_os = "macos")))]
//! Coverage for the Linux notifier command builder. The command is only inspected,
//! never spawned, so no desktop notification is shown.

use super::*;

fn program_and_args(command: &tokio::process::Command) -> (String, Vec<String>) {
    let std_command = command.as_std();
    let program = std_command
        .get_program()
        .to_str()
        .expect("utf-8 program")
        .to_owned();
    let args = std_command
        .get_args()
        .map(|arg| arg.to_str().expect("utf-8 arg").to_owned())
        .collect();
    (program, args)
}

#[test]
fn linux_platform_command_runs_notify_send_with_title_and_message_after_separator() {
    let cases: [(&str, &str, bool); 4] = [
        (TITLE, "PUBLIC-message \u{b7} 12 chars", false),
        (TITLE, "PUBLIC-message \u{b7} 12 chars", true),
        // Leading dashes stay as positional text because they follow "--".
        ("PUBLIC-title", "--PUBLIC-not-an-option", false),
        ("PUBLIC \"quoted\" title", "PUBLIC back\\slash", true),
    ];
    for (title, message, sound) in cases {
        let command = platform_command(title, message, sound).expect("command builds");
        let (program, args) = program_and_args(&command);
        assert_eq!(program, "notify-send", "case {title:?}/{message:?}/{sound}");
        assert_eq!(
            args,
            vec![
                "--app-name=Alcuin".to_owned(),
                "--".to_owned(),
                title.to_owned(),
                message.to_owned(),
            ],
            "case {title:?}/{message:?}/{sound}"
        );
        // No environment is injected: the text travels only as arguments.
        assert_eq!(command.as_std().get_envs().count(), 0);
    }
}

#[test]
fn linux_platform_command_ignores_the_sound_flag() {
    let silent = platform_command("PUBLIC-title", "PUBLIC-body", false).expect("silent builds");
    let loud = platform_command("PUBLIC-title", "PUBLIC-body", true).expect("loud builds");
    assert_eq!(program_and_args(&silent), program_and_args(&loud));
}
