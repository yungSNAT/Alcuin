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
    # Windows PowerShell 5.1 may not offer TLS 1.2 by default, which GitHub requires.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
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
    try {
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
        foreach ($bin in 'alcuin.exe', 'alcuin-mcp.exe') {
            $target = Join-Path $dir $bin
            # A running binary, such as an MCP client's alcuin-mcp.exe, can be renamed but not
            # overwritten, so the old one is moved aside first.
            if (Test-Path $target) {
                Remove-Item "$target.old" -Force -ErrorAction SilentlyContinue
                Move-Item $target "$target.old" -Force
            }
            Copy-Item "$tmp\$name\$bin" $target
            # Gone unless it is still running; the next install removes it then.
            Remove-Item "$target.old" -Force -ErrorAction SilentlyContinue
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
                    $key.SetValue('Path', (($entries + $dir) -join ';'), 'ExpandString')
                    # Setting and clearing a variable this way tells open programs, such as
                    # Explorer, that the environment changed, so new terminals see the Path.
                    [Environment]::SetEnvironmentVariable('ALCUIN_INSTALL_NOTIFY', '1', 'User')
                    [Environment]::SetEnvironmentVariable('ALCUIN_INSTALL_NOTIFY', $null, 'User')
                    $env:Path = "$dir;$env:Path"
                    "Added $dir to your user Path; open a new terminal to use it elsewhere."
                }
            }
        } finally {
            $key.Close()
        }
    } finally {
        Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
    }
}
