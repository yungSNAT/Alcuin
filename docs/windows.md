# Windows

Alcuin runs natively on 64-bit Windows (x86_64). [Install](install.md#windows) unpacks it under `%LOCALAPPDATA%\Programs\Alcuin`. The commands here are for PowerShell; `cmd.exe` differs only in quoting.

## The browser

Alcuin looks for Chrome (stable or Canary) under `%LOCALAPPDATA%`, `%ProgramFiles%` and `%ProgramFiles(x86)%`, then for Edge. It does not look for Brave, Vivaldi or other Chromium-based browsers: set `CHROME_PATH` to the executable. `alcuin bridge doctor` shows what it found:

```powershell
$env:CHROME_PATH = 'C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe'
alcuin bridge doctor
```

The `Chrome:` line names the browser. With none found, it reads `Chrome: not found`, `Problems:` lists 'No Chrome installation detected' and the command exits 1. To keep `CHROME_PATH`, add it to your user environment variables, or put `browser.chromePath` in your user config ([Configuration](configuration.md)).

By default, Windows browser runs use Alcuin's own profile, where you sign in once ([Quickstart](quickstart.md#3-sign-in-once)). Alcuin does not copy cookies from your everyday browser on Windows, and `bridge doctor` says so on its `Cookies:` line.

## Where Alcuin keeps its files

Sessions, settings and the browser profile live in `%USERPROFILE%\.oracle`. To keep them elsewhere, set `ORACLE_HOME_DIR`; `bridge doctor` shows the config file it then reads:

```powershell
$env:ORACLE_HOME_DIR = "$env:USERPROFILE\alcuin-data"
alcuin bridge doctor | Select-String 'Config:'
```

## Quoting file patterns

`--file` matches its own patterns, so pass them quoted. In PowerShell, single or double quotes both work, and a leading `!` excludes:

```powershell
alcuin --engine browser --dry-run summary -p "Review the parser please" --file 'src/**/*.rs' '!src/**/*_test.rs'
```

In `cmd.exe`, use double quotes. Single quotes are passed on as part of the pattern, which then matches nothing: Alcuin prints 'No files matched the provided --file patterns.' and exits 1. The same applies to `--agent "*"` in the README's skills command.

## Notifications

A finished run shows a Windows toast through Windows PowerShell 5.1, silent unless you pass `--notify-sound`. [Notifications](notifications.md) covers turning them off.

## WSL

Alcuin does not launch a browser from inside WSL: a browser run there stops with 'Launching Chrome under WSL: this build does not support it. Run Alcuin on the Windows host instead.' Run `alcuin.exe` on Windows instead. To use the Windows browser from Linux, including WSL, run `alcuin serve` on Windows and connect to it ([A browser on another machine](remote.md)).

<sub>Checked with the v0.1.0 Windows binary in a scratch profile on 15 November 2026: `bridge doctor` without a browser it knows and with `CHROME_PATH` set to Brave, `ORACLE_HOME_DIR`, and the pattern quoting in PowerShell 7 and `cmd.exe`. The toast and the WSL refusal were read from the source and its tests, not run.</sub>
