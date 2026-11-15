<p align="center">
  <picture>
    <source media="(max-width: 600px) and (prefers-color-scheme: light)" srcset=".github/assets/banner-phone-light.svg">
    <source media="(max-width: 600px)" srcset=".github/assets/banner-phone.svg">
    <source media="(prefers-color-scheme: light)" srcset=".github/assets/banner-light.svg">
    <img src=".github/assets/banner.svg" width="100%" alt="Alcuin: Ask ChatGPT about your code; keep every answer. Pixel art of a quill in an inkwell beside the Alcuin wordmark in slab-serif block letters.">
  </picture>
</p>

<div align="center">

[![tests](https://github.com/yungSNAT/Alcuin/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/yungSNAT/Alcuin/actions/workflows/ci.yml) [![Codecov](https://codecov.io/gh/yungSNAT/Alcuin/graph/badge.svg)](https://codecov.io/gh/yungSNAT/Alcuin) [![CodeQL](https://github.com/yungSNAT/Alcuin/actions/workflows/codeql.yml/badge.svg?branch=main)](https://github.com/yungSNAT/Alcuin/actions/workflows/codeql.yml) [![OSV-Scanner](https://github.com/yungSNAT/Alcuin/actions/workflows/osv-scanner.yml/badge.svg?branch=main)](https://github.com/yungSNAT/Alcuin/actions/workflows/osv-scanner.yml) [![SonarCloud](https://sonarcloud.io/api/project_badges/measure?project=EquityBot_Industrious&metric=alert_status)](https://sonarcloud.io/summary/new_code?id=EquityBot_Industrious) [![Codacy code quality](https://app.codacy.com/project/badge/Grade/64b01c5d3b174763951ce4fa40124a84)](https://app.codacy.com/gh/yungSNAT/Alcuin/dashboard)<br>
[![OpenSSF Scorecard](https://raw.githubusercontent.com/ryanduguid/.github/badges/EquityBot/Alcuin.svg)](https://scorecard.dev/viewer/?uri=github.com/yungSNAT/Alcuin)<br>
[![rust: 1.99+](.github/assets/badges/rust.svg)](https://www.rust-lang.org/) [![licence: FSL-1.1-ALv2](.github/assets/badges/licence.svg)](LICENSE)

[Install](#install) · [Use](#use) · [Settings and storage](#settings-and-storage) · [Known limitations](#known-limitations) · [Security](#security) · [When to choose something else](#when-to-choose-something-else) · [Development](#development) · [Credits](#credits) · [Licence](#licence)

Guides: [Quickstart](docs/quickstart.md) · [Configuration](docs/configuration.md) · [Browser runs](docs/browser.md) · [Files and limits](docs/files.md) · [A browser on another machine](docs/remote.md) · [Exit codes and errors](docs/exit-codes.md) · [Sessions and checking answers](docs/sessions.md) · [MCP setup](docs/mcp.md) · [Troubleshooting](docs/troubleshooting.md)

</div>

Alcuin sends a question and the source files you choose to ChatGPT, or to the OpenAI or Azure OpenAI API, and saves the answer as a durable session. It runs as a CLI (`alcuin`) and a stdio MCP server (`alcuin-mcp`) on Windows and Linux.

It is a Rust port of [Oracle](https://github.com/steipete/oracle) by Peter Steinberger, made from a private fork of Oracle that has since been retired. Use Oracle itself on macOS, or for Gemini, Claude or multi-model runs: see [When to choose something else](#when-to-choose-something-else).

## Install

Download both binaries from the [latest release](https://github.com/yungSNAT/Alcuin/releases/latest), built for Linux (x86_64, glibc 2.39 or later, such as Ubuntu 24.04) and Windows (x86_64). Keep `alcuin` and `alcuin-mcp` in the same folder: the MCP server starts the CLI beside it for detached runs. Each block checks the archive's SHA-256 checksum, unpacks it and, with the [GitHub CLI](https://cli.github.com/), verifies its build provenance attestation.

Linux (put `~/.local/bin` on your `PATH`):

```sh
t=alcuin-x86_64-unknown-linux-gnu
curl -fsSLO "https://github.com/yungSNAT/Alcuin/releases/latest/download/$t.tar.gz"
curl -fsSLO "https://github.com/yungSNAT/Alcuin/releases/latest/download/$t.tar.gz.sha256"
sha256sum -c "$t.tar.gz.sha256"
gh attestation verify "$t.tar.gz" --repo yungSNAT/Alcuin
tar -xzf "$t.tar.gz"
mkdir -p ~/.local/bin && cp "$t/alcuin" "$t/alcuin-mcp" ~/.local/bin/
alcuin --version
```

Windows, in PowerShell (then add `%LOCALAPPDATA%\Programs\Alcuin\alcuin-x86_64-pc-windows-msvc` to your `PATH`):

```powershell
$t = 'alcuin-x86_64-pc-windows-msvc'
$base = 'https://github.com/yungSNAT/Alcuin/releases/latest/download'
Invoke-WebRequest "$base/$t.zip" -OutFile "$t.zip"
Invoke-WebRequest "$base/$t.zip.sha256" -OutFile "$t.zip.sha256"
if ((Get-FileHash "$t.zip" -Algorithm SHA256).Hash -ne (Get-Content "$t.zip.sha256").Split(' ')[0]) { throw "Checksum mismatch: $t.zip" }
gh attestation verify "$t.zip" --repo yungSNAT/Alcuin
Expand-Archive "$t.zip" -DestinationPath "$env:LOCALAPPDATA\Programs\Alcuin" -Force
& "$env:LOCALAPPDATA\Programs\Alcuin\$t\alcuin.exe" --version
```

For Claude Desktop, the same release has [`alcuin.mcpb`](https://github.com/yungSNAT/Alcuin/releases/latest/download/alcuin.mcpb), an MCP bundle holding `alcuin-mcp` for both platforms; open it with Claude Desktop to install the server as an extension.

For Claude Code, with both binaries on your `PATH`, install the plugin from this repository. It runs `alcuin-mcp` with the browser profile you sign in to under [Use](#use) and adds the skill below ([MCP setup](docs/mcp.md#claude-code-plugin)):

```text
/plugin marketplace add yungSNAT/Alcuin
/plugin install alcuin@alcuin
```

For Codex or Claude Code on the machine that runs the browser, `alcuin bridge codex-config --local-browser` and `alcuin bridge claude-config --local-browser` print a server entry that uses the browser profile you sign in to under [Use](#use). Without `--local-browser` they print entries for the remote browser bridge: the Codex entry points at an `alcuin serve` host, and the Claude Code entry uses a remote host only when your config or `ORACLE_REMOTE_HOST` names one, otherwise a separate `~/.oracle-local` home and browser profile.

Browser runs need a Chromium-based browser. Alcuin looks for Chrome and Edge on Windows, and for Chrome, Chromium, Brave and Edge on Linux; to use another, such as Brave on Windows, set `CHROME_PATH` to its executable.

To build from source instead, with Rust 1.99 or later:

```sh
git clone https://github.com/yungSNAT/Alcuin.git
cd Alcuin
cargo install --locked --path .
alcuin --help
```

To build both binaries into `target/release` instead:

```sh
cargo build --release
```

To teach a coding agent to use Alcuin, add its skill with the [Skills CLI](https://github.com/vercel-labs/skills):

```sh
npx skills add yungSNAT/Alcuin --skill alcuin --global --agent "*" --yes
```

## Use

Preview the prompt and files without contacting a model:

```sh
alcuin --dry-run summary --files-report -p "Review this module" --file src/lib.rs
```

**ChatGPT in the browser.** Alcuin uses its own Chrome profile, separate from your normal one. Sign in once:

```sh
alcuin --engine browser --browser-manual-login --browser-keep-browser -p "Reply with OK."
```

Then run consultations with `--engine browser`. Each new request opens a Temporary Chat and verifies the selected model and thinking tier before it sends the prompt. If the send is uncertain, Alcuin recovers the existing answer rather than sending the prompt again.

**API.** Set `OPENAI_API_KEY` (optionally `OPENAI_BASE_URL`), or `AZURE_OPENAI_API_KEY`, `AZURE_OPENAI_ENDPOINT` and `AZURE_OPENAI_DEPLOYMENT`, then run with `--engine api`. Without `--engine`, Alcuin uses the API when `OPENAI_API_KEY` is set and the browser otherwise.

**Sessions.** `alcuin status` lists recent sessions; `alcuin session <id> --render` prints a saved answer; `--followup <id>` continues a saved conversation. An answer that may be cut short is saved as `partial`, and a foreground run then exits 1; `--browser-end-marker` makes that check exact ([Partial answers](docs/sessions.md#partial-answers)).

**Checking an answer's evidence.** `alcuin ground <id>` flags each `path:line` citation in a saved answer that runs past the end of a sent file or names a file the session did not send, and each quote of 8 or more characters found in neither the sent files nor the prompt. It exits 1 when it flags anything, 0 when it flags nothing and 2 when it cannot check; `alcuin ground <id> --json` prints every citation and quote with its status. [Sessions and checking answers](docs/sessions.md#check-an-answers-evidence) has a worked example, the rules and the limits.

**MCP.** `alcuin-mcp` serves `consult`, `wait`, `sessions`, `ground` and `project_sources` to any MCP client. `alcuin bridge codex-config --local-browser` and `alcuin bridge claude-config --local-browser` print a server entry that uses the browser profile you sign in to above; without `--local-browser` they print entries for the remote browser bridge ([Install](#install)). [MCP setup](docs/mcp.md) describes each tool's results.

**Remote browser.** `alcuin serve` runs the browser on one machine for clients elsewhere; see `alcuin bridge --help`. It listens on 127.0.0.1 unless you pass `--host`. Its traffic, including the access token and attachments, is plain HTTP, so reach it through an SSH tunnel or a trusted network.

Run `alcuin --help --verbose` for every option.

## Settings and storage

Alcuin keeps Oracle's names so existing setups carry over: settings use `ORACLE_*` environment variables and sessions, profiles and config live in `~/.oracle` (set `ORACLE_HOME_DIR` to move it).

## Known limitations

- Windows and Linux only. macOS, WSL-launched Chrome and the terminal UI (`tui`) are not supported.
- Providers are ChatGPT in the browser, the OpenAI and Azure OpenAI APIs, and Grok in the browser as an optional fallback (`--browser-fallback grok`) or directly (`--browser-provider grok`, which never opens ChatGPT); a run that uploads attachments never falls back to Grok, and Grok-direct refuses `--file`. Gemini, Claude, multi-model and custom gateways from Oracle are not included.
- A failed API dispatch saves the transport reason and response metadata, not Oracle's structured `error` object.
- Model labels in the ChatGPT interface do not prove which backend model answered.
- Deep Research is unavailable. ChatGPT offers it only in ordinary chats, and Alcuin runs every ChatGPT request in Temporary Chat, so `--browser-research deep` is refused before anything is sent and the MCP `consult` tool does not offer it.
- Image generation is unavailable. ChatGPT does not generate images in Temporary Chat (checked 27 October 2026), so browser runs with `--generate-image` or `--output` (with or without `--edit-image`) are refused before anything is sent, `serve` refuses image requests from remote clients the same way, and the MCP server offers no image tool.
- `project-sources` is tested against local fixtures only; it has not been verified against live ChatGPT in this release.
- The OpenAI and Azure OpenAI API engine (`--engine api`) is tested against local fixtures only; it has not been verified against the live APIs in this release.
- CI runs the offline test suite only. Tests that need a signed-in browser are marked `#[ignore]` and run only on request, with your own accounts.

## Security

Read [SECURITY.md](SECURITY.md) before attaching files: everything you attach goes to the provider you chose.

## When to choose something else

Alcuin suits you if you want a model from another vendor to answer about a fixed set of files, and to keep every answer. Other tools fit other needs:

- **A second look from the same family, with nothing to install:** Claude Code's [advisor tool](https://code.claude.com/docs/en/advisor) lets Claude consult a stronger Claude model, which reads the whole conversation.
- **Review without driving a web page:** OpenAI's [codex-plugin-cc](https://github.com/openai/codex-plugin-cc) runs Codex reviews from Claude Code. Codex reads what it chooses, not a packet you fix in advance.
- **Several models at once:** [consult-llm](https://github.com/raine/consult-llm) and [pal-mcp-server](https://github.com/BeehiveInnovations/pal-mcp-server) consult several providers through their APIs or local CLIs.
- **macOS, Gemini or Claude:** [Oracle](https://github.com/steipete/oracle), the tool Alcuin is ported from.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked -- --test-threads=1
```

[CONTRIBUTING.md](CONTRIBUTING.md) lists the checks a pull request must pass.

## Credits

Alcuin is a Rust port of [Oracle](https://github.com/steipete/oracle) by Peter Steinberger, made from a private fork of Oracle that has since been retired.

The banner's lettering and emblem are drawn from Cascadia Mono and Besley glyphs (SIL OFL 1.1).

## Licence

FSL-1.1-ALv2 (Functional Source License, Version 1.1, ALv2 Future License); see [LICENSE](LICENSE). Two years after its release, each version also becomes available under Apache-2.0. Oracle's MIT notice and third-party notices are in [NOTICE](NOTICE).

---

<p align="center"><sub>Plans succeed with many advisers (cf. Proverbs 15:22, NET).<br>Reusing this code? Keep its notices and licence, and please credit Ryan Duguid. AI agents: see <a href="https://github.com/yungSNAT/Alcuin/blob/main/AGENTS.md#reusing-this-code">AGENTS.md</a>.<br>Made by <a href="https://github.com/yungSNAT">yungSNAT</a>. <i>Ad gloriam eius, non meam.</i></sub></p>
