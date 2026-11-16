#!/usr/bin/env bash
# R0698 probe: start headless Chrome as the native test does (no HOME), then with HOME set,
# 5 times each, and report startup time and Fontconfig lines on stderr.
set -u
C=$(command -v google-chrome || command -v google-chrome-stable || command -v chromium)
echo "chrome: $C $($C --version 2>/dev/null)"
for mode in nohome home nohome home nohome home nohome home nohome home; do
  d=$(mktemp -d)
  mkdir -p "$d/p"
  start=$(date +%s%N)
  if [ "$mode" = nohome ]; then
    env -i PATH=/usr/bin:/bin "$C" --headless=new --no-first-run --no-default-browser-check \
      --user-data-dir="$d/p" --remote-debugging-port=0 about:blank 2> "$d/err" &
  else
    env -i PATH=/usr/bin:/bin HOME="$d" "$C" --headless=new --no-first-run --no-default-browser-check \
      --user-data-dir="$d/p" --remote-debugging-port=0 about:blank 2> "$d/err" &
  fi
  pid=$!
  ready=no
  for _ in $(seq 1 300); do
    if [ -s "$d/p/DevToolsActivePort" ]; then ready=yes; break; fi
    sleep 0.05
  done
  ms=$(( ($(date +%s%N) - start) / 1000000 ))
  kill "$pid" 2> /dev/null; wait "$pid" 2> /dev/null
  echo "$mode ready=$ready ms=$ms fontconfig_lines=$(grep -c -i fontconfig "$d/err")"
  grep -i -m 2 fontconfig "$d/err" | cut -c1-200
  rm -rf "$d"
done
