# Installs alcuin.exe and alcuin-mcp.exe on Windows (x86_64), in PowerShell:
#   irm https://raw.githubusercontent.com/yungSNAT/Alcuin/main/install.ps1 | iex
# It checks the archive's SHA-256 checksum and that the binary runs before it replaces anything,
# then adds the folder to your user Path.
# ALCUIN_VERSION picks a release tag such as v0.1.0 (default: the latest release).
# ALCUIN_INSTALL_DIR is the folder for both binaries (default: %LOCALAPPDATA%\Programs\Alcuin).
# ALCUIN_DOWNLOAD_URL replaces the release download folder, for a mirror.
# ALCUIN_NO_MODIFY_PATH=1 leaves your user Path alone.
& {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    if (-not [Environment]::Is64BitOperatingSystem) {
        throw 'alcuin install: releases are built for 64-bit Windows only.'
    }
    $version = if ($env:ALCUIN_VERSION) { $env:ALCUIN_VERSION } else { 'latest' }
    $base = if ($env:ALCUIN_DOWNLOAD_URL) {
        $env:ALCUIN_DOWNLOAD_URL
    } elseif ($version -eq 'latest') {
        'https://github.com/yungSNAT/Alcuin/releases/latest/download'
    } else {
        "https://github.com/yungSNAT/Alcuin/releases/download/$version"
    }
    $dir = if ($env:ALCUIN_INSTALL_DIR) { $env:ALCUIN_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\Alcuin' }
    $name = 'alcuin-x86_64-pc-windows-msvc'
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid())
    New-Item -ItemType Directory $tmp | Out-Null
    $protocols = [Net.ServicePointManager]::SecurityProtocol
    try {
        # Windows PowerShell 5.1 may not offer TLS 1.2 by default, which GitHub requires. The
        # setting is the whole process's, so it is put back afterwards.
        [Net.ServicePointManager]::SecurityProtocol = $protocols -bor [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest "$base/$name.zip" -OutFile "$tmp\$name.zip" -UseBasicParsing
        Invoke-WebRequest "$base/$name.zip.sha256" -OutFile "$tmp\$name.zip.sha256" -UseBasicParsing
        $expected = (Get-Content "$tmp\$name.zip.sha256" -Raw).Trim().Split(' ')[0]
        # .NET rather than Get-FileHash and Expand-Archive, which live in modules that a
        # PowerShell started from another version's environment may fail to load.
        $sha256 = [Security.Cryptography.SHA256]::Create()
        $actual = [BitConverter]::ToString($sha256.ComputeHash([IO.File]::ReadAllBytes("$tmp\$name.zip"))).Replace('-', '')
        if (-not $expected -or $expected -ne $actual) {
            throw "alcuin install: checksum mismatch for $name.zip; nothing was installed."
        }
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        [IO.Compression.ZipFile]::ExtractToDirectory("$tmp\$name.zip", $tmp)
        $installed = & "$tmp\$name\alcuin.exe" --version
        if ($LASTEXITCODE -ne 0) {
            throw 'alcuin install: the downloaded alcuin.exe does not run here. Nothing was installed.'
        }
        New-Item -ItemType Directory -Force $dir | Out-Null
        $bins = 'alcuin.exe', 'alcuin-mcp.exe'
        $replaced = @()
        try {
            # Both binaries are copied beside their targets first, so a failed copy leaves the
            # old pair in place.
            foreach ($bin in $bins) {
                Copy-Item "$tmp\$name\$bin" (Join-Path $dir "$bin.new")
            }
            foreach ($bin in $bins) {
                $target = Join-Path $dir $bin
                # Left by an earlier install whilst it was running; removed once it is not.
                Get-ChildItem $dir -Filter "$bin.*.old" | Remove-Item -Force -ErrorAction SilentlyContinue
                # A running binary, such as an MCP client's alcuin-mcp.exe, can be renamed but not
                # overwritten, so the old one is moved aside first, under a name no earlier
                # install can still hold.
                $old = if (Test-Path $target) { "$target.$([Guid]::NewGuid().ToString('N')).old" }
                # Recorded first, so a failure from here on is undone.
                $replaced += , @($target, $old)
                if ($old) {
                    Move-Item $target $old
                }
                Move-Item "$target.new" $target
            }
        } catch {
            $failure = $_
            # Put back what was replaced, last first, so the old pair stays whole. Each is tried
            # even if another fails.
            [array]::Reverse($replaced)
            foreach ($pair in $replaced) {
                $target, $old = $pair
                try {
                    if (-not $old) {
                        Remove-Item $target -Force -ErrorAction SilentlyContinue
                    } elseif (Test-Path $old) {
                        # Move-Item -Force deletes the target even when the move then fails, so
                        # the new binary is moved aside instead, and back if the old one is held.
                        $new = "$target.$([Guid]::NewGuid().ToString('N')).old"
                        if (Test-Path $target) {
                            Move-Item $target $new
                        }
                        try {
                            Move-Item $old $target
                        } catch {
                            if (Test-Path $new) {
                                Move-Item $new $target
                            }
                            throw
                        }
                        Remove-Item $new -Force -ErrorAction SilentlyContinue
                    }
                } catch {
                    Write-Warning "alcuin install: the old $(Split-Path $target -Leaf) is in $old."
                }
            }
            throw $failure
        } finally {
            foreach ($bin in $bins) {
                Remove-Item (Join-Path $dir "$bin.new") -Force -ErrorAction SilentlyContinue
            }
        }
        foreach ($pair in $replaced) {
            if ($pair[1]) {
                # Gone unless it is still running; a later install removes it then.
                Remove-Item $pair[1] -Force -ErrorAction SilentlyContinue
            }
        }
        "Installed alcuin $installed and alcuin-mcp in $dir."
        # The registry value as stored: GetEnvironmentVariable would expand entries such as
        # %USERPROFILE%\bin, and SetEnvironmentVariable would save them expanded.
        $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
        try {
            $userPath = $key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
            $entries = @($userPath -split ';' | Where-Object { $_ })
            if ($entries -notcontains $dir) {
                if ($env:ALCUIN_NO_MODIFY_PATH -eq '1') {
                    "Add $dir to your Path to run alcuin from any terminal."
                } else {
                    # Kept as its own kind: REG_EXPAND_SZ, or REG_SZ if the user chose that.
                    $kind = if ($key.GetValueNames() -contains 'Path') { $key.GetValueKind('Path') } else { 'ExpandString' }
                    $key.SetValue('Path', (($entries + $dir) -join ';'), $kind)
                    # Setting and clearing a variable this way tells open programs, such as
                    # Explorer, that the environment changed, so new terminals see the Path. The
                    # name is new each time, so no variable of the user's is touched.
                    $notify = "ALCUIN_INSTALL_$([Guid]::NewGuid().ToString('N'))"
                    [Environment]::SetEnvironmentVariable($notify, '1', 'User')
                    [Environment]::SetEnvironmentVariable($notify, $null, 'User')
                    $env:Path = "$dir;$env:Path"
                    "Added $dir to your user Path; open a new terminal to use it elsewhere."
                }
            }
        } finally {
            $key.Close()
        }
    } finally {
        [Net.ServicePointManager]::SecurityProtocol = $protocols
        Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
    }
}
