# Quickstart

From download to a checked answer. Steps 3 and 4 need a signed-in ChatGPT account.

## 1. Install

Download and verify the latest release with the Linux or PowerShell block in the README's [Install](../README.md#install) section. Each block checks the archive's SHA-256 checksum and, with the [GitHub CLI](https://cli.github.com/), its build provenance attestation, then prints the version:

```sh
alcuin --version
```

Keep `alcuin` and `alcuin-mcp` in the same folder on your `PATH`.

## 2. Preview without contacting a model

```sh
alcuin --engine browser --dry-run summary -p "Review lib.rs for bugs" --file lib.rs
```

The preview lists the files that would be sent, with their sizes, and the model it would ask for. Nothing leaves your machine. Read [SECURITY.md](../SECURITY.md) before attaching real files: everything you attach goes to the provider.

## 3. Sign in once

Alcuin uses its own browser profile under `~/.oracle`, separate from your normal one. This opens it so you can sign in to ChatGPT, then sends a one-line test:

```sh
alcuin --engine browser --browser-manual-login --browser-keep-browser -p "Reply with OK."
```

Alcuin needs a Chromium-based browser. It looks for Chrome and Edge on Windows, and for Chrome, Chromium, Brave and Edge on Linux; to use another, such as Brave on Windows, set `CHROME_PATH` to its executable. See [Troubleshooting](troubleshooting.md#no-browser-found) if it finds none.

## 4. Ask your first question

```sh
alcuin --engine browser -p "Review lib.rs for bugs" --file lib.rs
```

Each request opens a Temporary Chat and checks the selected model and thinking level before it sends. The answer is saved as a session. A Pro run, the default, continues in a detached session: if the command returns or times out before the answer, attach with `alcuin session <id>` rather than running it again, so the prompt is not sent twice.

## 5. Find and check the answer

```sh
alcuin status
alcuin session <id> --render
alcuin ground <id>
```

`status` lists recent sessions with their IDs. `session <id> --render` prints a saved answer. `ground <id>` flags citations (`path:line`) that point past the end of a sent file or at a file that was not sent, and quotes found in neither the files nor the prompt. It exits 0 when it flags nothing, 1 when it flags something and 2 when it cannot check, such as for an unknown session. Add `--json` for every citation and quote with its status.

To continue a conversation, pass `--followup <id>` with a new `-p`.

## Next

- [Sessions and checking answers](sessions.md) to list, continue, recover and ground saved sessions.
- [MCP setup](mcp.md) to call Alcuin from Claude Desktop, Claude Code or Codex.
- [Troubleshooting](troubleshooting.md).
- `alcuin --help --verbose` for every option.

<sub>Checked with the v0.1.0 release binaries on 13 November 2026: steps 1, 2 and 5's `status`, and the error paths of `session` and `ground` for an unknown ID. Steps 3 and 4 and step 5 with a real session need a signed-in ChatGPT account and were not run.</sub>
