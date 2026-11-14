# Sessions and checking answers

Every run is saved as a session: its prompt, the files it sent, the answer and a log. Sessions live in `~/.oracle/sessions/<id>`; set `ORACLE_HOME_DIR` to move `~/.oracle`. The ID is a slug made from the prompt, such as `is-total-right` for `-p "Is total() right?"`, with `-2`, `-3` and so on added when it is taken.

## List sessions

```sh
alcuin status
```

```text
Recent Sessions
Status    Model         Mode    Timestamp           Chars    Cost  Slug
completed gpt-6-astra   api/fg  11/15/2026   7:44 AM    17  $0.003  is-total-right
completed gpt-6-astra   api/fg  11/15/2026   7:45 AM    25  $0.003  └─ which-line-should-change
```

`status` shows the last 24 hours, up to 100 sessions. `--hours <n>` and `--limit <n>` change the window and the cap, and `--all` shows every stored session. A follow-up is listed under the session it continues.

## Read a session

```sh
alcuin session is-total-right
```

This prints the session's status, model, prompt and answer. `--render` formats the answer's Markdown in a terminal, `--hide-prompt` leaves the prompt out, and `--path` prints where the session's files are instead. For a run still going, `session <id>` attaches to it and prints the answer when it arrives; attach rather than running the prompt again, so it is not sent twice.

## Continue or repeat a session

```sh
alcuin --followup is-total-right -p "Which line should change?"
alcuin restart is-total-right
```

`--followup <id>` sends a new prompt in the same conversation: an API run continues from the saved response, and a browser run continues in the saved ChatGPT conversation. The follow-up is a new session. `restart <id>` runs the original prompt and files again as a new session with the same options.

## Recover a browser answer

If a browser run stopped before Alcuin saved the answer, these read the session's ChatGPT tab without sending anything:

```sh
alcuin session <id> --harvest
alcuin session <id> --live
```

`--harvest` reads the tab once and saves the latest answer; `--live` follows the tab until the answer completes, stalls or the tab detaches. `--write-output <file>` also writes the answer to a file. If the tab is gone, Alcuin reopens the saved conversation unless you pass `--no-recover`.

## Partial answers

A run that saved an answer which may be cut ends as `partial`, with `response.status: incomplete` and the reason in `response.incompleteReason`. `--browser-end-marker` asks ChatGPT to finish with a line unique to the run, and saves any answer without it as `partial`. A foreground or `--wait` run saved as `partial` prints a notice and exits 1; the text is still kept in the session.

## Check an answer's evidence

`alcuin ground <id>` reads a session's answer and checks it against the files the session sent. Take a session that sent this 4-line `sample.rs`:

```rust
/// Adds every number from 1 to `limit`.
pub fn total(limit: u32) -> u32 {
    (1..limit).sum()
}
```

and got this answer:

```text
The loop in `sample.rs:3` stops one short, and `sample.rs:40` is wrong too.
The comment says "adds every number" but the code says "for n in 1..limit".
```

```sh
alcuin ground is-total-right
```

```text
Grounding: 2 references (1 grounded, 1 out of range, 0 not in the files sent); 2 quotes (0 found, 2 not found).
Out of range:
  answer line 1: sample.rs:40 (file has 4 lines)
Quotes not found in the files sent:
  answer line 2: 'adds every number'
  answer line 2: 'for n in 1..limit'
```

`sample.rs:3` is grounded. `sample.rs:40` runs past the end of the file. Neither quote is in the file: the comment says 'Adds every number', and quotes must match exactly, including case.

What it checks:

- **Citations:** each `path:line` or `path:start-end` that runs past the end of a sent file (`out-of-range`) or names a file the session did not send. Relative and home (`~/`) citations match any sent file whose path ends with them; absolute ones match only that file. A sent unified diff counts on both sides.
- **Quotes:** each quoted span of 8 or more characters found in neither the sent files nor the prompt. Fenced code blocks are skipped, as they usually propose code.
- **Which bytes:** the copy the session saved when it sent its files. When no copy remains, as after a browser run closes its tab cleanly, it reads the files as they are now, says so and lists any that changed since the session began.

It exits 1 when it flags anything, 0 when it flags nothing and 2 when it cannot check, such as for an unknown session. `--json` prints every citation and quote with its status (`grounded`, `out-of-range`, `host` and so on) and `found` for each quote, plus `sentFiles` (`saved` when it read the saved copy) and any `changedFiles`.

Limits: inline code counts as a quote, so a name the answer proposes is flagged; a quote that wraps across 2 lines of the answer is not checked; `file#L10` citations are ignored, as are names without an extension other than dotfiles and conventional ones such as `Dockerfile` and `Makefile`; a name such as `example.com:8080` reads as a host, listed with status `host` and not flagged, unless a sent file has that name.

<sub>Checked on 15 November 2026 with the v0.1.0 Windows binary against a local stand-in for the Azure OpenAI Responses API, so no model was contacted: `status`, `session`, `session --path`, `ground` and `ground --json` on a real session, `ground` on an unknown session (exit 2), `--followup` and `restart`. The outputs above are from that run. `--harvest`, `--live` and `--browser-end-marker` need a signed-in ChatGPT account and were not run; they are described from `alcuin session --help` and the README.</sub>
