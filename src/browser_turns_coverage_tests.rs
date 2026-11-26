use super::transcript;

#[test]
fn sequential_transcript_marks_empty_and_whitespace_only_answers() {
    let followups = [
        (" PUBLIC-first prompt ".into(), String::new()),
        ("PUBLIC-second prompt".into(), " PUBLIC-answer\n".into()),
    ];

    assert_eq!(
        transcript(" \t\n", &followups),
        concat!(
            "## Initial response\n\n### Answer\n\n_No text captured._",
            "\n\n## Follow-up 1\n\n### Prompt\n\nPUBLIC-first prompt",
            "\n\n### Answer\n\n_No text captured._",
            "\n\n## Follow-up 2\n\n### Prompt\n\nPUBLIC-second prompt",
            "\n\n### Answer\n\nPUBLIC-answer",
        )
    );
}
