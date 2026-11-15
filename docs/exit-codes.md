# Exit codes and errors

Scripts and agents that run `alcuin` can tell the outcome from its exit code. Errors go to stderr: a problem with an option starts with `error:`, and most others with `✖`.

| Code | Meaning |
| --- | --- |
| 0 | Success |
| 1 | An error, including an unknown option or a value it does not accept, a refused request, a run already going, a failed run, a run saved as `partial` and drift found by `docs check` |
| 2 | `alcuin ground` could not check the session, for example because it does not exist |
| 130 | You pressed Ctrl-C before an answer was saved |

`alcuin ground` exits 1 when it flags a citation or quote, as well as on an error ([Check an answer's evidence](sessions.md#check-an-answers-evidence)).

## Options

An unknown option, or a value an option does not accept, prints the problem and `(use --help for usage)`, and sends nothing:

```sh
alcuin --engine bogus -p "Review main.rs please"
```

A run with no prompt prints 'Prompt is required.' on stdout and the help on stderr, and exits 1.

## Refused requests

Alcuin refuses some requests before anything is sent, and says why:

- options from Oracle for features this build leaves out, such as `--retain-hours`, and the matching config keys ([Configuration](configuration.md#when-a-file-cannot-be-used));
- models other than Latest (GPT-6), and browser thinking levels other than Pro and Extra High ([Browser runs](browser.md#model-and-thinking-level));
- Deep Research and image generation in the browser, which ChatGPT does not offer in Temporary Chat;
- files that are too large or look like credentials ([Files and limits](files.md#checks-before-sending)).

## A run already going

If the same prompt is already running, a new run prints a notice on stdout and exits 1, rather than sending it twice. Attach to the running session with `alcuin session <id>`, or pass `--force` to start another anyway.

## Ctrl-C

Pressing Ctrl-C while a browser run owns its browser prints 'Cancelled.' and exits 130 without stopping the run, so its session stays open: attach to it with `alcuin session <id>`. Ctrl-C during `alcuin session`, `alcuin status` and the other commands that wait also exits 130.

## Partial answers

A foreground or `--wait` run whose answer may be cut short is saved as `partial`, prints a notice and exits 1; the text is still saved ([Partial answers](sessions.md#partial-answers)).
