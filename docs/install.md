# Install, update and remove

Alcuin ships as 2 binaries, `alcuin` (the CLI) and `alcuin-mcp` (the MCP server), for Linux and Windows on x86_64, and, from the first release after v0.1.0, for Linux on aarch64 (64-bit Arm) and macOS on Apple silicon. Keep them in the same folder: the MCP server starts the CLI beside it for detached runs. Every release also has `alcuin.mcpb`, an MCP bundle for Claude Desktop ([MCP setup](mcp.md#claude-desktop)).

## One command

The install scripts download the latest release, check its SHA-256 checksum, check that `alcuin` runs on your machine and only then put both binaries in place. Running one again updates Alcuin.

Linux, or macOS from the first release after v0.1.0, into `~/.local/bin` (it says so if that folder is not on your `PATH`):

```sh
curl -fsSL https://raw.githubusercontent.com/yungSNAT/Alcuin/main/install.sh | sh
```

Windows, in PowerShell, into `%LOCALAPPDATA%\Programs\Alcuin`, which it adds to your user `Path`:

```powershell
irm https://raw.githubusercontent.com/yungSNAT/Alcuin/main/install.ps1 | iex
```

Both read these environment variables:

| Variable | Effect |
| --- | --- |
| `ALCUIN_VERSION` | A release tag such as `v0.1.0`, in place of the latest release |
| `ALCUIN_INSTALL_DIR` | The folder for both binaries |
| `ALCUIN_DOWNLOAD_URL` | A folder holding the release archive and its `.sha256` file, such as a mirror; it must be HTTPS, though `install.ps1` also takes plain HTTP from this machine itself |
| `ALCUIN_NO_MODIFY_PATH` | `1` leaves your Windows user `Path` alone |

The scripts check the checksum published beside the archive, which catches a damaged or swapped download but not a release replaced together with its checksum. The steps below also verify the build provenance attestation with the [GitHub CLI](https://cli.github.com/).

## Linux

The release is built for glibc 2.39 or later, such as Ubuntu 24.04. To see your version:

```sh
ldd --version | head -1
```

Download the archive and its checksum, check it, verify its build provenance with the [GitHub CLI](https://cli.github.com/), and copy both binaries to `~/.local/bin`:

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

On a 64-bit Arm machine, set `t=alcuin-aarch64-unknown-linux-gnu` instead (from the first release after v0.1.0). `sha256sum -c` prints `alcuin-x86_64-unknown-linux-gnu.tar.gz: OK`. If `alcuin --version` is not found, put `~/.local/bin` on your `PATH`, for example in `~/.bashrc`:

```sh
export PATH="$HOME/.local/bin:$PATH"
command -v alcuin alcuin-mcp
```

## macOS

From the first release after v0.1.0, `alcuin-aarch64-apple-darwin` runs on Apple silicon; there is no Intel build. Follow the Linux steps with `t=alcuin-aarch64-apple-darwin` and `shasum -a 256 -c` in place of `sha256sum -c`. Put `~/.local/bin` on your `PATH` in `~/.zshrc`.

## Windows

In PowerShell, download the archive and its checksum, check it, verify its provenance and unpack it under `%LOCALAPPDATA%\Programs\Alcuin`:

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

The folder holds `alcuin.exe`, `alcuin-mcp.exe`, `LICENSE`, `NOTICE` and `README.md`. To use it in the current PowerShell window:

```powershell
$env:Path = "$env:LOCALAPPDATA\Programs\Alcuin\alcuin-x86_64-pc-windows-msvc;$env:Path"
alcuin --version
```

To keep it, add the same folder to your user `Path` under Windows' 'Edit environment variables for your account', then open a new terminal. [Windows](windows.md) covers the browser, file locations and quoting.

## Claude Code, Codex and other agents

With both binaries on your `PATH`, [MCP setup](mcp.md) installs the Claude Code plugin or prints a server entry for Claude Code or Codex, and [Coding agents](agents.md) adds the `alcuin` skill.

## From source

With Rust 1.99 or later, the README's [Install](../README.md#install) section builds and installs both binaries from a clone with Cargo.

## Update

Run the same block again: it downloads the latest release over the old files. Then check the version against the [latest release](https://github.com/yungSNAT/Alcuin/releases/latest):

```sh
alcuin --version
gh release view --repo yungSNAT/Alcuin --json tagName --jq .tagName
```

`alcuin --version` prints the bare version, such as `0.1.0`, and the release tag adds a `v`. Sessions, settings and the browser sign-in are kept: they live in `~/.oracle`, not beside the binaries.

## Remove

Delete the binaries. On Linux:

```sh
rm ~/.local/bin/alcuin ~/.local/bin/alcuin-mcp
```

On Windows:

```powershell
Remove-Item -Recurse "$env:LOCALAPPDATA\Programs\Alcuin"
```

This leaves `~/.oracle` (`%USERPROFILE%\.oracle` on Windows), which holds your saved sessions, settings and Alcuin's browser profile with its ChatGPT sign-in. If you set `ORACLE_HOME_DIR`, the sessions and settings are in that folder instead. Delete it too only if you no longer want those sessions.

<sub>Checked on 15 November 2026: both blocks downloaded v0.1.0, passed the checksum and attestation checks and printed `0.1.0`, the Linux one on glibc 2.44 and the Windows one in PowerShell 7 with `%LOCALAPPDATA%` pointed at a scratch folder; the `PATH` lines, the version check against the release tag and both removals were run there too.</sub>
