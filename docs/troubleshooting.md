# Troubleshooting

## No browser found

```sh
alcuin bridge doctor
```

On a machine without a browser Alcuin knows, this reports `Chrome: not found` and the problem 'No Chrome installation detected', and exits 1. The MCP server refuses a browser `consult` with 'Browser engine unavailable: no Chrome installation found and CHROME_PATH is unset.'

Alcuin looks for Chrome and Edge on Windows, and for Chrome, Chromium, Brave and Edge on Linux. For another Chromium-based browser, set `CHROME_PATH`; `bridge doctor` then names it on its `Chrome:` line. In PowerShell, for Brave:

```powershell
$env:CHROME_PATH = 'C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe'
alcuin bridge doctor
```

## A run stopped or timed out

Do not run the prompt again: the session is saved, and a second run sends it twice. Find it and attach:

```sh
alcuin status
alcuin session <id>
```

`alcuin session <id> --harvest` re-reads the session's browser tab and saves the latest answer; `--live` follows the tab until the answer completes, stalls or detaches. If the send itself was uncertain, Alcuin recovers the existing answer rather than sending again.

## The answer is marked `partial`

A saved answer that may be cut short ends as `partial`, with `response.status: incomplete` and the reason in `response.incompleteReason`. A foreground run saved as `partial` prints a notice and exits 1; the text is still in the session. To make the check exact, pass `--browser-end-marker`: ChatGPT is asked to finish with a line unique to the run, and an answer without it is saved as `partial`. It needs one turn and a local browser.

## 'is not supported in this build'

Some options carried over from Oracle are refused with the reason, for example:

```sh
alcuin --retain-hours 1 status
```

prints '--retain-hours is not supported in this build: session pruning is not included; stored sessions are kept for recovery.' and exits 1. Deep Research (`--browser-research deep`) and image generation are refused before anything is sent, because ChatGPT does not offer them in Temporary Chat. See [Known limitations](../README.md#known-limitations).

## Sessions, settings and the browser profile

They live in `~/.oracle`, the name Oracle uses, so existing setups carry over. Set `ORACLE_HOME_DIR` to move them. A server entry printed without `--local-browser` uses a different profile, under `~/.oracle-local`, so a sign-in made with the CLI does not apply to it; see [MCP setup](mcp.md#claude-code-and-codex-on-the-browsers-machine).

## A remote browser

`alcuin serve` speaks plain HTTP, access token and attachments included. Reach it through an SSH tunnel or a trusted network; see [SECURITY.md](../SECURITY.md).

<sub>Checked with the v0.1.0 release on 13 November 2026: `bridge doctor` without a known browser and with `CHROME_PATH` set to Brave, `status`, the `--retain-hours` and Deep Research refusals, and the help for `session --harvest` and `--live`. Attaching to, harvesting or following a real session needs a signed-in ChatGPT account and was not run.</sub>
