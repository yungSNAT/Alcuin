# MCP setup

`alcuin-mcp` is a stdio MCP server. It offers 5 tools:

| Tool | What it does |
| --- | --- |
| `consult` | Runs an Alcuin session through the API or ChatGPT in the browser, with the files you attach |
| `project_sources` | Lists a ChatGPT Project's Sources tab, or previews which files an upload would add |
| `sessions` | Lists recent sessions or fetches one by ID |
| `wait` | Waits for a session to finish without polling |
| `ground` | Checks a saved answer's citations and quotes against the files its session sent, as [`alcuin ground`](sessions.md#check-an-answers-evidence) does |

`consult` and `wait` return the whole saved answer in `output` and the last 4,000 UTF-16 units of the session log in `log`, with `modelSelection` and `thinkingSelection` naming the model and effort the browser verified. A client that declares the MCP Tasks extension gets `consult` back as a task to poll with `tasks/get` and stop with `tasks/cancel`; other clients wait for the result as before. `ground` returns what `alcuin ground --json` prints, plus `sessionId` and `flagged`; a flagged answer is a result, not an error. When the session kept no copy of what it sent, the files `ground` reads must lie inside the server's working directory, as `consult`'s must, unless `ORACLE_MCP_ALLOW_EXTERNAL_INPUT=1` is set. The server shares sessions, settings and the browser profile with the CLI, so sign in once with the CLI first ([Quickstart](quickstart.md#3-sign-in-once)). [MCP tool reference](mcp-tools.md) lists every tool's inputs and outputs.

## Claude Desktop

Download [`alcuin.mcpb`](https://github.com/yungSNAT/Alcuin/releases/latest/download/alcuin.mcpb) from the latest release and open it with Claude Desktop, which installs the server as an extension. The bundle holds `alcuin` and `alcuin-mcp` for Windows and Linux (x86_64). A bundle chooses its server command by operating system, not by processor, so the macOS (Apple silicon) and Linux aarch64 builds would need a launcher that picks the binary by processor; the bundle does not have one yet. On macOS or Linux on Arm, install the archive for your platform ([Install](install.md)) and add the `mcpServers` block that `alcuin bridge claude-config --local-browser` prints ([below](#claude-code-and-codex-on-the-browsers-machine)) to your MCP client's configuration (for Claude Desktop on macOS, `claude_desktop_config.json`), with `command` set to the full path of `alcuin-mcp`, since an app started from the desktop may not have your shell's `PATH`. To check the bundle first:

```sh
curl -fsSLO https://github.com/yungSNAT/Alcuin/releases/latest/download/alcuin.mcpb
curl -fsSLO https://github.com/yungSNAT/Alcuin/releases/latest/download/alcuin.mcpb.sha256
sha256sum -c alcuin.mcpb.sha256
gh attestation verify alcuin.mcpb --repo yungSNAT/Alcuin
```

## Claude Code plugin

With `alcuin` and `alcuin-mcp` on your `PATH`, add this repository as a plugin marketplace and install the plugin, in Claude Code:

```text
/plugin marketplace add yungSNAT/Alcuin
/plugin install alcuin@alcuin
```

The plugin runs `alcuin-mcp` with `ORACLE_ENGINE=browser` and the default session folder and browser profile, the same server as `alcuin bridge claude-config --local-browser` prints, and adds the `alcuin` and `alcuin-review` skills ([Coding agents](agents.md#add-the-skill)). `/mcp` lists it as `plugin:alcuin:alcuin`; if it shows as failed, check first that `alcuin-mcp` is on the `PATH` Claude Code started with. The plugin's files come from this repository's default branch; the binaries are the ones you downloaded. For another session folder or profile, use the entry printed below instead.

## Codex in one command

With `alcuin` and `alcuin-mcp` on your `PATH`, add the server to Codex with the Codex CLI:

```sh
codex mcp add alcuin --env ORACLE_ENGINE=browser -- alcuin-mcp
```

Codex writes it to `~/.codex/config.toml` as `[mcp_servers.alcuin]`, and `codex mcp list` shows it. It runs `alcuin-mcp` with `ORACLE_ENGINE=browser` and the default session folder and browser profile, the server the Claude Code plugin runs. For another session folder or profile, use the entry printed below instead.

## Claude Code and Codex on the browser's machine

With `alcuin` and `alcuin-mcp` on your `PATH`, print an entry that uses the browser profile the CLI signs in to:

```sh
alcuin bridge claude-config --local-browser
alcuin bridge codex-config --local-browser
```

The Claude Code entry is an `mcpServers` block, the format of a project's `.mcp.json`. The Codex entry is a `[mcp_servers.alcuin]` table for `~/.codex/config.toml`. Both run `alcuin-mcp` with `ORACLE_ENGINE=browser` and your `ORACLE_HOME_DIR` and browser profile paths.

Without `--local-browser`, the same commands print entries for the remote browser bridge: the Codex entry points at an `alcuin serve` host (`ORACLE_REMOTE_HOST`, `ORACLE_REMOTE_TOKEN`), and the Claude Code entry uses a remote host only when your config or `ORACLE_REMOTE_HOST` names one, otherwise a separate browser profile under `~/.oracle-local`, which is not the one the CLI signs in to. `alcuin bridge --help` describes the bridge.

## Browser

Alcuin looks for Chrome and Edge on Windows, and for Chrome, Chromium, Brave and Edge on Linux. To use another Chromium-based browser, such as Brave on Windows, set `CHROME_PATH` to its executable in the entry's `env`; the server passes it on to the runs it starts. `alcuin bridge doctor` shows the browser it found.

<sub>Checked with the v0.1.0 release on 13 November 2026: the bundle's checksum and attestation, both config commands with and without `--local-browser`, `bridge doctor` with and without `CHROME_PATH`, and the server's `tools/list`. Installing the bundle in Claude Desktop and calling the tools from a client need a signed-in ChatGPT account and were not run. On 15 November 2026 the plugin was installed from a local copy of this repository into an empty Claude Code 2.1.322 configuration, which listed its skill and server; the server connected with the v0.1.0 `alcuin-mcp`. On 16 November 2026 the `codex mcp add` command above wrote that entry into an empty Codex CLI 0.169.0 home, and `codex mcp list` listed it as enabled.</sub>
