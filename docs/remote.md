# A browser on another machine

ChatGPT runs need a signed-in browser. If it lives on another machine, such as a Windows desktop you sign in on while your code is on a Linux server, run `alcuin serve` there and point the other machine's runs at it.

Its traffic, including the access token and your attachments, is plain HTTP. Keep it on the machine it runs on and reach it through an SSH tunnel, or use a network you trust; see [SECURITY.md](../SECURITY.md).

## On the machine with the browser

Sign in once ([Quickstart](quickstart.md#3-sign-in-once)) and put a long random token in a file, then start the service:

```sh
alcuin serve --manual-login --port 9473 --token-file ~/.oracle/serve-token
```

| Option | Effect |
| --- | --- |
| `--host <address>` | Interface to listen on. The default, 127.0.0.1, accepts this machine only |
| `--port <number>` | Port to listen on (default: a random free port) |
| `--token-file <path>` | Read the access token from a file. `--token <value>` passes it directly, but other users can see it in the process list; without either, a random token is made |
| `--manual-login` | Use Alcuin's own signed-in profile; `--manual-login-profile-dir <path>` chooses another |
| `--max-concurrent-runs <count>` | Run several requests at once and queue the rest, up to the profile's tab limit. By default one run at a time, and a request that arrives during a run is refused |
| `--max-queued-runs <count>` | Requests that may wait in that queue (default 8) |

`alcuin bridge host` does the same with defaults for a bridge: it listens on 127.0.0.1:9473 with a generated token, and writes the address and token to `~/.oracle/bridge-connection.json` for the other machine. `--ssh <user@host>` keeps an SSH reverse tunnel open to that machine, so the service can stay on 127.0.0.1; `--print` prints the connection string, token included.

## On the machine with your code

Point a run at the service with its address and token:

```sh
alcuin --engine browser --remote-host 127.0.0.1:9473 --remote-token "$TOKEN" -p "Review the parser" --file src/parser.rs
```

`ORACLE_REMOTE_HOST` and `ORACLE_REMOTE_TOKEN` set the same for every run. To keep them in your user config, copy `bridge-connection.json` across and run:

```sh
alcuin bridge client --connect bridge-connection.json
```

This writes `browser.remoteHost` and `browser.remoteToken` to `~/.oracle/config.json` and checks that the service answers. `--no-write-config` checks without writing, and `--no-test` writes without checking. A config value wins over the environment variables, and a project's `.oracle/config.json` cannot set either ([Configuration](configuration.md#what-a-project-file-may-set)).

`alcuin bridge doctor` reports the remote host and token it would use, where each came from, and whether the browser prerequisites are met.

## Recover a remote result

The service saves each run's result. `alcuin recover-remote <run-id> --host <host:port> --token <token>` copies a saved result into a new local session without sending the prompt again, for example when the connection dropped before the answer arrived.

## MCP clients

`alcuin bridge codex-config` and `alcuin bridge claude-config` print an `alcuin-mcp` entry that uses the service; [MCP setup](mcp.md) covers them.
