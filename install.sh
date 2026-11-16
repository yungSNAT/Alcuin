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
	# A signal exits through the EXIT trap, which cleans up and, between the renames below,
	# puts the old binaries back.
	trap 'exit 1' HUP INT TERM
	curl --proto '=https' --tlsv1.2 -fsSL "$base/$name.tar.gz" -o "$tmp/$name.tar.gz" || fail "could not download $base/$name.tar.gz"
	curl --proto '=https' --tlsv1.2 -fsSL "$base/$name.tar.gz.sha256" -o "$tmp/$name.tar.gz.sha256" || fail "could not download $base/$name.tar.gz.sha256"
	expected="$(cut -d ' ' -f 1 "$tmp/$name.tar.gz.sha256")"
	actual="$(sum "$tmp/$name.tar.gz" | cut -d ' ' -f 1)"
	[ -n "$expected" ] && [ "$expected" = "$actual" ] || fail "checksum mismatch for $name.tar.gz; nothing was installed."
	tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
	installed="$("$tmp/$name/alcuin" --version)" ||
		fail "the downloaded alcuin does not run here; it needs glibc 2.39 or later (ldd --version shows yours). Nothing was installed."

	mkdir -p "$dir"
	# Both binaries are copied beside their targets, and the old ones linked aside, before either
	# is replaced, so a failed copy leaves the old pair in place. A rename then replaces each, even
	# one that is running, such as an MCP client's alcuin-mcp; if the second fails, or a signal
	# stops the script between them, the first old one is put back.
	keep_old=
	renaming=
	restore() {
		renaming=
		for bin in alcuin alcuin-mcp; do
			if [ -f "$dir/.$bin.old" ]; then
				# Still the old one when its own rename never ran.
				cmp -s "$dir/.$bin.old" "$dir/$bin" && continue
				if ! mv -f "$dir/.$bin.old" "$dir/$bin"; then
					keep_old=1
					echo "alcuin install: the old $bin is in $dir/.$bin.old." >&2
				fi
			elif [ -f "$dir/$bin" ] && cmp -s "$dir/$bin" "$tmp/$name/$bin"; then
				rm -f "$dir/$bin"
			fi
		done
	}
	trap '[ -z "$renaming" ] || restore; rm -rf "$tmp"; rm -f "$dir/.alcuin.new" "$dir/.alcuin-mcp.new"; [ -n "$keep_old" ] || rm -f "$dir/.alcuin.old" "$dir/.alcuin-mcp.old"' EXIT
	for bin in alcuin alcuin-mcp; do
		cp "$tmp/$name/$bin" "$dir/.$bin.new" || fail "could not write to $dir; nothing was installed."
		chmod 755 "$dir/.$bin.new"
		if [ -f "$dir/$bin" ]; then
			ln -f "$dir/$bin" "$dir/.$bin.old" 2>/dev/null || cp -p "$dir/$bin" "$dir/.$bin.old" ||
				fail "could not keep a copy of $dir/$bin; nothing was installed."
		fi
	done
	renaming=1
	if ! { mv -f "$dir/.alcuin.new" "$dir/alcuin" && mv -f "$dir/.alcuin-mcp.new" "$dir/alcuin-mcp"; }; then
		restore
		fail "could not replace the binaries in $dir; the old ones were put back."
	fi
	renaming=
	echo "Installed alcuin $installed and alcuin-mcp in $dir."
	case ":$PATH:" in
	*":$dir:"*) ;;
	*) echo "Add $dir to your PATH, for example in ~/.bashrc: export PATH=\"$dir:\$PATH\"" ;;
	esac
}

main "$@"
