# Linux

The release runs on x86_64 Linux with glibc 2.39 or later, such as Ubuntu 24.04 ([Install](install.md#linux)). There is no arm64 build; on other machines, build from source.

## The browser

Alcuin looks for a browser in this order, and uses the first it finds:

1. `CHROME_PATH`, if it names an executable file;
2. the browsers your desktop's application entries start (Chrome and Chromium);
3. fixed paths, among them `/usr/bin/google-chrome`, `/usr/bin/chromium`, `/usr/bin/brave-browser`, `/usr/bin/microsoft-edge`, the same browsers under `/snap/bin` and `/opt/google/chrome/chrome`;
4. those browsers' names on your `PATH`.

`alcuin bridge doctor` names the one it found on its `Chrome:` line:

```sh
alcuin bridge doctor
CHROME_PATH=/usr/bin/chromium alcuin bridge doctor
```

## Signing in

Pass `--browser-manual-login`, or set `browser.manualLogin` to `true` in [your config](configuration.md), to use Alcuin's own profile, where you sign in once ([Quickstart](quickstart.md#3-sign-in-once)). Copying the sign-in from your everyday browser's cookies is off unless you pass `--browser-cookie-sync`; a `--dry-run` preview says which applies on its `Cookies:` line.

## A machine without a desktop

A browser run opens a visible window, so it needs a graphical session. `--browser-headless` runs the browser without one, but ChatGPT or Cloudflare may block a headless browser, as the preview warns:

```sh
alcuin --engine browser --browser-headless --dry-run summary -p "Review main.rs please" --file src/main.rs
```

On a server, run the browser on a machine with a desktop and connect to it with `alcuin serve` ([A browser on another machine](remote.md)), or use the API (`--engine api`).

## WSL

Alcuin does not launch a browser inside WSL; run it on the Windows host instead ([Windows](windows.md#wsl)).

## Notifications

A finished run shows a desktop notification through `notify-send`, unless you are connected over SSH ([Notifications](notifications.md)).

## Where Alcuin keeps its files

Sessions, settings and the browser profile live in `~/.oracle`; set `ORACLE_HOME_DIR` to move them.

<sub>Checked with a build of `main` on Linux (glibc 2.44, Chromium) on 15 November 2026: `bridge doctor` with and without `CHROME_PATH`, and the headless preview. The discovery order, the notification and the WSL refusal were read from the source and its tests.</sub>
