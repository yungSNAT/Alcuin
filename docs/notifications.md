# Notifications

When a run saves its answer, Alcuin shows a desktop notification, so you can leave a long Pro run in the background. Notifications are on by default, except when `CI` or `SSH_CONNECTION` is set, and the sound is off.

## What it shows

The title is 'Alcuin🧿 finished'. The text is the session's slug (or its ID), the cost for an API run, the answer's length in characters and the first 140 characters of the answer, with code blocks, links and other Markdown removed. **The notification shows the start of the answer**, so turn notifications off when others can see your screen.

On Windows it is a toast from Windows PowerShell; on Linux, `notify-send`. A notification that cannot be shown, for example because `notify-send` is not installed, prints `(notify skipped: ...)` on stderr. It never changes the run's result or exit code, and Alcuin waits at most 10 seconds for it.

## Turning them on or off

Each setting takes the first of these that is set:

| | Notification | Sound |
| --- | --- | --- |
| Option | `--notify`, `--no-notify` | `--notify-sound`, `--no-notify-sound` |
| Environment | `ORACLE_NOTIFY` | `ORACLE_NOTIFY_SOUND` |
| [Config](configuration.md) | `notify.enabled` | `notify.sound` |
| Default | On, but off when `CI` or `SSH_CONNECTION` is set | Off |

The environment variables take `1`, `true`, `yes` or `on`, and `0`, `false`, `no` or `off`. `ORACLE_DISABLE_NOTIFICATIONS=1` turns notifications off whatever the other settings say. In `~/.oracle/config.json`, to turn them off for every run:

```json
{ "notify": { "enabled": false } }
```

A run decides when it starts, and a detached run keeps that decision. `alcuin restart` uses the settings saved with the session it repeats.
