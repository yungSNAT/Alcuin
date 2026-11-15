#!/bin/sh
# Installs the alcuin and alcuin-mcp release binaries on Linux (x86_64):
#   curl -fsSL https://raw.githubusercontent.com/yungSNAT/Alcuin/main/install.sh | sh
# It checks the archive's SHA-256 checksum and that the binary runs before it replaces anything.
# ALCUIN_VERSION picks a release tag such as v0.1.0 (default: the latest release).
# ALCUIN_INSTALL_DIR is the folder for both binaries (default: ~/.local/bin).
# ALCUIN_DOWNLOAD_URL replaces the release download folder, for a mirror.
set -eu

# The whole script is one function, run on its last line, so a download cut short runs nothing.
main() {
	fail() {
		echo "alcuin install: $*" >&2
		exit 1
	}

	[ "$(uname -s)" = Linux ] || fail "this script installs the Linux build; on Windows run install.ps1 (docs/install.md)."
	case "$(uname -m)" in
	x86_64 | amd64) ;;
	*) fail "releases are built for x86_64 only; build from source instead (docs/install.md#from-source)." ;;
	esac
	command -v curl >/dev/null 2>&1 || fail "curl is required."
	if command -v sha256sum >/dev/null 2>&1; then
		sum() { sha256sum "$1"; }
	elif command -v shasum >/dev/null 2>&1; then
		sum() { shasum -a 256 "$1"; }
	else
		fail "sha256sum or shasum is required to check the download."
	fi

	version="${ALCUIN_VERSION:-latest}"
	if [ -n "${ALCUIN_DOWNLOAD_URL:-}" ]; then
		base="$ALCUIN_DOWNLOAD_URL"
	elif [ "$version" = latest ]; then
		base="https://github.com/yungSNAT/Alcuin/releases/latest/download"
	else
		base="https://github.com/yungSNAT/Alcuin/releases/download/$version"
	fi
	dir="${ALCUIN_INSTALL_DIR:-$HOME/.local/bin}"
	name=alcuin-x86_64-unknown-linux-gnu

	tmp="$(mktemp -d)"
	trap 'rm -rf "$tmp"' EXIT
	curl -fsSL "$base/$name.tar.gz" -o "$tmp/$name.tar.gz" || fail "could not download $base/$name.tar.gz"
	curl -fsSL "$base/$name.tar.gz.sha256" -o "$tmp/$name.tar.gz.sha256" || fail "could not download $base/$name.tar.gz.sha256"
	expected="$(cut -d ' ' -f 1 "$tmp/$name.tar.gz.sha256")"
	actual="$(sum "$tmp/$name.tar.gz" | cut -d ' ' -f 1)"
	[ -n "$expected" ] && [ "$expected" = "$actual" ] || fail "checksum mismatch for $name.tar.gz; nothing was installed."
	tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
	installed="$("$tmp/$name/alcuin" --version)" ||
		fail "the downloaded alcuin does not run here; it needs glibc 2.39 or later (ldd --version shows yours). Nothing was installed."

	mkdir -p "$dir"
	# A rename replaces a binary that is running, such as an MCP client's alcuin-mcp; a copy would not.
	for bin in alcuin alcuin-mcp; do
		cp "$tmp/$name/$bin" "$dir/.$bin.new"
		chmod 755 "$dir/.$bin.new"
		mv -f "$dir/.$bin.new" "$dir/$bin"
	done
	echo "Installed alcuin $installed and alcuin-mcp in $dir."
	case ":$PATH:" in
	*":$dir:"*) ;;
	*) echo "Add $dir to your PATH, for example in ~/.bashrc: export PATH=\"$dir:\$PATH\"" ;;
	esac
}

main "$@"
