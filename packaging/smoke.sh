#!/usr/bin/env bash
# Runs a packaged alcuin and alcuin-mcp in a scratch home: the version, an offline dry run and the
# MCP handshake with its tool list. Usage: bash packaging/smoke.sh <folder holding both> <version>
set -euo pipefail
dir=$1
version=$2
exe=""
if [ -f "$dir/alcuin.exe" ]; then exe=.exe; fi
home=$(mktemp -d)
cd "$home"
export HOME="$home" USERPROFILE="$home" ORACLE_HOME_DIR="$home/.oracle" ORACLE_DISABLE_NOTIFICATIONS=1

test "$("$dir/alcuin$exe" --version | tr -d '\r')" = "$version"

printf 'pub fn smoke() {}\n' > smoke.rs
"$dir/alcuin$exe" --dry-run summary --files-report -p "Smoke test" --file smoke.rs | tee dry-run.txt
grep -qF "Alcuin ($version)" dry-run.txt
grep -qF "smoke.rs" dry-run.txt

printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' |
  timeout 60 "$dir/alcuin-mcp$exe" > mcp.jsonl
grep -qF "\"serverInfo\":{\"name\":\"alcuin-mcp\",\"version\":\"$version\"}" mcp.jsonl
for tool in consult project_sources sessions wait; do
  grep -qF "{\"name\":\"$tool\"" mcp.jsonl
done
echo "smoke test passed: $dir"
