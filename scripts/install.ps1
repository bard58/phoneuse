# Install agentphone on Windows without a Rust toolchain.
#
# Usage:
#   irm https://raw.githubusercontent.com/bard58/phoneuse/rust-rewrite/scripts/install.ps1 | iex
#
# The script:
#   1. Downloads agentphone.exe from the latest GitHub Release of bard58/phoneuse.
#   2. Puts it in $env:LOCALAPPDATA\agentphone\bin and adds that folder to the
#      user PATH (user scope, no admin).
#   3. Checks for the Apple Devices app and the Apple Mobile Device service.
#   4. Runs "agentphone doctor".
#
# It runs no downloaded code other than "agentphone.exe --version" and
# "agentphone.exe doctor". Compatible with Windows PowerShell 5.1.

& {
    $ErrorActionPreference = 'Stop'
    Set-StrictMode -Version 2

    $repo = 'bard58/phoneuse'
    $binDir = Join-Path $env:LOCALAPPDATA 'agentphone\bin'
    $exePath = Join-Path $binDir 'agentphone.exe'
    $storeUrl = 'ms-windows-store://pdp/?productid=9NP83LWLPZ9K'
    $storeWebUrl = 'https://apps.microsoft.com/detail/9NP83LWLPZ9K'

    function Write-Step([string]$Text) { Write-Host "==> $Text" -ForegroundColor Cyan }
    function Write-Warn([string]$Text) { Write-Host "warning: $Text" -ForegroundColor Yellow }

    function Show-ManualHelp {
        Write-Host ''
        Write-Host 'No release binary is available. Use one of these methods:'
        Write-Host '  1. Build with Rust 1.88 or later:'
        Write-Host "       cargo install --git https://github.com/$repo --branch rust-rewrite ap-cli"
        Write-Host '  2. Download the Windows artifact of a CI run from:'
        Write-Host "       https://github.com/$repo/actions"
        Write-Host "     Then put agentphone.exe in $binDir and run this script again."
    }

    function Test-PeFile([string]$Path) {
        $stream = [System.IO.File]::OpenRead($Path)
        try {
            $buf = New-Object byte[] 2
            $n = $stream.Read($buf, 0, 2)
            return ($n -eq 2 -and $buf[0] -eq 0x4D -and $buf[1] -eq 0x5A)
        } finally {
            $stream.Dispose()
        }
    }

    # PowerShell 5.1 does not always enable TLS 1.2. GitHub needs it.
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    } catch { }
    $ProgressPreference = 'SilentlyContinue'

    # Select the target for this CPU.
    $arch = $env:PROCESSOR_ARCHITEW6432
    if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
    if ($arch -eq 'ARM64') { $target = 'aarch64-pc-windows-msvc' } else { $target = 'x86_64-pc-windows-msvc' }

    # 1. Find the latest release asset.
    Write-Step "Looking for the latest release of $repo ($target)"
    $release = $null
    try {
        $release = Invoke-RestMethod -UseBasicParsing -Headers @{ 'User-Agent' = 'agentphone-install' } `
            -Uri "https://api.github.com/repos/$repo/releases/latest"
    } catch {
        Write-Warn "cannot get the latest release: $($_.Exception.Message)"
    }

    $asset = $null
    if ($release) {
        $asset = @($release.assets | Where-Object { $_.name -like "agentphone-*-$target.zip" }) | Select-Object -First 1
        if (-not $asset) { Write-Warn "release $($release.tag_name) has no asset for $target." }
    }

    if (-not $asset) {
        if (Test-Path $exePath) {
            Write-Warn "no download. The script keeps the existing $exePath."
        } else {
            Show-ManualHelp
            return
        }
    } else {
        $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ('agentphone-install-' + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Force -Path $tmp | Out-Null
        try {
            $zip = Join-Path $tmp $asset.name
            Write-Step "Downloading $($asset.name)"
            Invoke-WebRequest -UseBasicParsing -Headers @{ 'User-Agent' = 'agentphone-install' } `
                -Uri $asset.browser_download_url -OutFile $zip

            $extract = Join-Path $tmp 'x'
            Expand-Archive -Path $zip -DestinationPath $extract -Force
            $found = @(Get-ChildItem -Path $extract -Recurse -File -Filter 'agentphone.exe') | Select-Object -First 1
            if (-not $found) {
                Write-Warn 'the archive does not contain agentphone.exe.'
                Show-ManualHelp
                return
            }
            if (-not (Test-PeFile $found.FullName)) {
                Write-Warn 'the downloaded agentphone.exe is not a Windows executable (no MZ header). The script stops.'
                return
            }

            New-Item -ItemType Directory -Force -Path $binDir | Out-Null
            Copy-Item -Path $found.FullName -Destination $exePath -Force
            Write-Step "Installed $exePath ($($release.tag_name))"
        } finally {
            Remove-Item -Recurse -Force -Path $tmp -ErrorAction SilentlyContinue
        }
    }

    if (-not (Test-PeFile $exePath)) {
        Write-Warn "$exePath is not a Windows executable (no MZ header). The script stops."
        return
    }

    # 2. Add the folder to the user PATH.
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $parts = @()
    if ($userPath) { $parts = @($userPath -split ';' | Where-Object { $_ }) }
    $onPath = $parts | Where-Object { $_.TrimEnd('\') -ieq $binDir.TrimEnd('\') }
    if (-not $onPath) {
        $newPath = (@($parts) + $binDir) -join ';'
        [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
        Write-Step "Added $binDir to the user PATH. Open a new terminal to use it."
    } else {
        Write-Step "$binDir is already on the user PATH."
    }
    if (($env:Path -split ';') -notcontains $binDir) { $env:Path = "$env:Path;$binDir" }

    & $exePath --version

    # 3. Check for the Apple Devices app and the Apple Mobile Device service.
    Write-Step 'Checking for the Apple Mobile Device service (needed for iPhone)'
    $service = Get-Service -Name 'Apple Mobile Device Service' -ErrorAction SilentlyContinue
    $app = $null
    try { $app = Get-AppxPackage -Name 'AppleInc.AppleDevices' -ErrorAction SilentlyContinue } catch { }
    if ($service -or $app) {
        if ($service) { Write-Host "Apple Mobile Device service: $($service.Status)" }
        if ($app) { Write-Host "Apple Devices app: $($app.Version)" }
    } else {
        Write-Warn 'the Apple Devices app is not installed. For an iPhone, install it from the Microsoft Store:'
        Write-Host "  $storeWebUrl"
        Write-Host "  or run: Start-Process '$storeUrl'"
        Write-Host '  You do not need it for Android.'
    }

    # 4. Run the doctor.
    Write-Step 'Running agentphone doctor'
    & $exePath doctor
    Write-Host ''
    Write-Host 'Next step: run "agentphone" in a new terminal. The setup wizard starts on the first run.'
}
