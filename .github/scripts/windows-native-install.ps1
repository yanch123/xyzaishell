# Run only on a fresh GitHub Windows Server runner. This is not Windows 10/11 QA.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:CI -ne 'true' -or $env:RUNNER_OS -ne 'Windows') { throw 'Disposable Windows CI only' }
if (-not $env:PACKAGE_DIR -or -not $env:TAURI_DRIVER) { throw 'Package and native driver setup required' }
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$evidence = Join-Path $env:RUNNER_TEMP 'xyzaishell-windows-smoke'
$upgrade = -not [string]::IsNullOrEmpty($env:UPGRADE_BASELINE_DIR)
if ($upgrade) { $evidence = Join-Path $evidence 'version-upgrade' }
New-Item -ItemType Directory -Path $evidence -Force | Out-Null
Start-Transcript -Path (Join-Path $evidence 'install.txt') | Out-Null
$fixture = Join-Path $env:RUNNER_TEMP ('xyz-native-' + [guid]::NewGuid().ToString('N'))
$install = Join-Path $fixture 'installed-app'
$finalFingerprint = $null
try {
    New-Item -ItemType Directory -Path $fixture | Out-Null
    Write-Output "New synthetic directory security before protection: $((Get-Acl -LiteralPath $fixture).Sddl)"
    # Only this new disposable fixture gets a private inheritable current-user ACL.
    $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    & icacls $fixture /inheritance:r /grant:r "*${sid}:(OI)(CI)F"
    if ($LASTEXITCODE -ne 0) { throw 'Could not protect synthetic fixture directory' }
    & icacls $fixture /setowner "*$sid"
    if ($LASTEXITCODE -ne 0) { throw 'Could not set synthetic fixture owner' }
    [IO.File]::WriteAllText((Join-Path $fixture '.xyz-native-fixture'), "synthetic disposable fixture`n")
    $env:APPDATA = Join-Path $fixture 'config'
    $env:LOCALAPPDATA = Join-Path $fixture 'local'
    $env:XYZAI_MODEL_CONFIG = $null
    New-Item -ItemType Directory -Path $env:APPDATA,$env:LOCALAPPDATA | Out-Null
    $configRoot = Join-Path $env:APPDATA 'xyzaishell'
    New-Item -ItemType Directory -Path $configRoot | Out-Null
    Write-Output "New synthetic config security before protection: $((Get-Acl -LiteralPath $configRoot).Sddl)"
    # ProfileStore requires the actual config root to have a protected DACL;
    # a private inherited ACL alone is insufficient. No existing user dir is edited.
    & icacls $configRoot /inheritance:r /grant:r "*${sid}:(OI)(CI)F"
    if ($LASTEXITCODE -ne 0) { throw 'Could not protect synthetic application config root' }
    & icacls $configRoot /setowner "*$sid"
    if ($LASTEXITCODE -ne 0) { throw 'Could not set synthetic application config owner' }
    foreach ($path in @($fixture, $configRoot)) {
        $acl = Get-Acl -LiteralPath $path
        if ($acl.GetOwner([Security.Principal.SecurityIdentifier]).Value -ne $sid -or -not $acl.AreAccessRulesProtected) {
            throw 'Synthetic directory owner/protected ACL does not match the application boundary'
        }
        Write-Output "Synthetic directory security: $path $($acl.Sddl)"
    }
    $application = Join-Path $install 'xyzaishell.exe'
    $packages = @(Get-ChildItem -LiteralPath $env:PACKAGE_DIR -Filter '*.exe')
    if ($packages.Count -ne 1) { throw 'Expected exactly one NSIS installer' }
    $package = $packages[0]
    & python (Join-Path $repo '.github/scripts/artifacts.py') verify-package --platform windows-x64 --installers $env:PACKAGE_DIR
    if ($LASTEXITCODE -ne 0) { throw 'Package provenance does not match the checked-out source' }
    $checksum = Get-Content -LiteralPath (Join-Path $env:PACKAGE_DIR 'SHA256SUMS-windows-x64.txt') |
        Where-Object { $_ -match ('^[a-f0-9]{64}  ' + [regex]::Escape($package.Name) + '$') }
    if (@($checksum).Count -ne 1) { throw 'Missing unique installer checksum' }
    if ((Get-FileHash -LiteralPath $package.FullName -Algorithm SHA256).Hash.ToLowerInvariant() -ne $checksum.Substring(0,64)) {
        throw 'Installer checksum mismatch'
    }
    $candidateVersion = (Get-Content -Raw -LiteralPath (Join-Path $repo 'package.json') | ConvertFrom-Json).version
    function Assert-InstalledVersion([string]$Expected) {
        if (-not (Test-Path -LiteralPath $application)) { throw 'Installed binary is missing' }
        $actual = (Get-Item -LiteralPath $application).VersionInfo.ProductVersion
        if ($actual -notmatch ('^' + [regex]::Escape($Expected) + '(?:\.0)?$')) { throw "Installed version $actual does not match $Expected" }
        Write-Output "PASS: installed executable version $actual"
    }
    function Invoke-Installer([string]$Executable, [string]$Arguments) {
        $process = Start-Process -FilePath $Executable -ArgumentList $Arguments -PassThru
        if (-not $process.WaitForExit(180000)) {
            $process.Kill()
            throw 'Synthetic installer operation timed out'
        }
        if ($process.ExitCode -ne 0) { throw "Installer operation failed: $($process.ExitCode)" }
    }
    function Get-ConfigFingerprint {
        $result = @{}
        foreach ($name in @('model.toml','connections.toml')) {
            $path = Join-Path $env:APPDATA "xyzaishell/$name"
            $result[$name] = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash
        }
        $result['history.sqlite3'] = (Get-FileHash -LiteralPath (Join-Path $env:LOCALAPPDATA 'xyzaishell/state/history.sqlite3') -Algorithm SHA256).Hash
        return $result
    }
    function Assert-ConfigFingerprint($Expected) {
        $actual = Get-ConfigFingerprint
        foreach ($name in $Expected.Keys) {
            if ($Expected[$name] -ne $actual[$name]) { throw "Installer changed $name" }
        }
        Write-Output 'PASS: synthetic model/profile/history config hashes preserved'
    }
    function Invoke-NativeUi([string]$Phase, [string]$Label, [string]$Version) {
        & python (Join-Path $repo '.github/scripts/native-ui.py') --application $application `
            --fixture $fixture --evidence (Join-Path $evidence $Label) --phase $Phase --application-version $Version
        if ($LASTEXITCODE -ne 0) { throw "Native installed-app UI check failed: $Label" }
    }
    Get-CimInstance Win32_OperatingSystem | Select-Object Caption,Version,BuildNumber | Format-List
    Write-Output "Source commit: $(& git -C $repo rev-parse HEAD)"
    if ($upgrade) {
        $baseline = & python (Join-Path $repo '.github/scripts/upgrade_baseline.py') verify --platform windows-x64 --directory $env:UPGRADE_BASELINE_DIR
        if ($LASTEXITCODE -ne 0 -or @($baseline).Count -ne 1) { throw 'Pinned upgrade baseline verification failed' }
        Invoke-Installer $baseline "/S /D=$install"
        Assert-InstalledVersion '0.1.1'
        & python (Join-Path $repo '.github/scripts/upgrade_baseline.py') verify --platform windows-x64 --directory $env:UPGRADE_BASELINE_DIR --application $application
        if ($LASTEXITCODE -ne 0) { throw 'Installed baseline executable is not the verified v0.1.1' }
        & (Join-Path $repo '.github/scripts/install-webview-driver.ps1') -Evidence $evidence
        Invoke-NativeUi 'seed' 'baseline-v0.1.1-install-and-restart' '0.1.1'
        $beforeUpgrade = Get-ConfigFingerprint
    }
    # NSIS /D is last; no uninstall/data reset between the old and new installer.
    Invoke-Installer $package.FullName "/S /D=$install"
    Assert-InstalledVersion $candidateVersion
    if ($upgrade) { Assert-ConfigFingerprint $beforeUpgrade }
    foreach ($resource in @('THIRD-PARTY-NOTICES.txt','RUST-STANDARD-LIBRARY.html','runtime-inventory.json','sources/option-ext-0.2.0.crate')) {
        $path = Join-Path $install "third-party/$resource"
        if (-not (Test-Path -LiteralPath $path) -or (Get-Item -LiteralPath $path).Length -eq 0) {
            throw "Missing installed notice/source: $resource"
        }
        Get-FileHash -LiteralPath $path -Algorithm SHA256 |
            Format-List | Out-File -FilePath (Join-Path $evidence 'installed-notices.sha256') -Append -Encoding utf8
    }
    # The real installer provisions WebView2 when missing. Only now select the
    # exact matching signed driver rather than assuming the runner has it.
    & (Join-Path $repo '.github/scripts/install-webview-driver.ps1') -Evidence $evidence
    if ($upgrade) {
        Invoke-NativeUi 'upgrade' 'upgraded-v0.1.2' $candidateVersion
    } else {
    Invoke-NativeUi 'seed' 'first-install-and-restart' $candidateVersion
    $before = Get-ConfigFingerprint
    Invoke-Installer $package.FullName "/S /D=$install"
    Assert-ConfigFingerprint $before
    Invoke-NativeUi 'preserved' 'same-version-reinstall' $candidateVersion
    $before = Get-ConfigFingerprint
    Invoke-Installer (Join-Path $install 'uninstall.exe') '/S'
    # NSIS may copy its uninstaller into TEMP; wait for its child to finish.
    $until = [DateTime]::UtcNow.AddSeconds(30)
    while (((Test-Path -LiteralPath $application) -or (Test-Path -LiteralPath (Join-Path $install 'uninstall.exe'))) -and [DateTime]::UtcNow -lt $until) { Start-Sleep -Milliseconds 200 }
    if (Test-Path -LiteralPath $application) { throw 'Uninstaller left the application binary' }
    if (Test-Path -LiteralPath (Join-Path $install 'uninstall.exe')) { throw 'Uninstaller did not finish cleanup' }
    Assert-ConfigFingerprint $before
    Invoke-Installer $package.FullName "/S /D=$install"
    Assert-ConfigFingerprint $before
    Invoke-NativeUi 'preserved' 'uninstall-and-reinstall' $candidateVersion
    }
    $finalFingerprint = Get-ConfigFingerprint
    if ($upgrade) {
        'PASS: pinned v0.1.1 installed and seeded through real UI; direct v0.1.2 upgrade retained profile/history hashes, text and stable IDs.' | Set-Content -LiteralPath (Join-Path $evidence 'result.txt') -Encoding utf8
    } else {
    @(
        'PASS: actual NSIS install and native UI/IPC synthetic profile/chat persistence.'
        'PASS: same-version reinstall and uninstall/reinstall preserve config hashes and restored text.'
        'NOT TESTED: Windows 10/11, version upgrade, IME/clipboard, SmartScreen UX, live provider, SSH via UI.'
    ) | Set-Content -LiteralPath (Join-Path $evidence 'result.txt') -Encoding utf8
    }
} finally {
    # The runner is discarded after this job. Remove only the fixture created above.
    try {
        try {
            $uninstaller = Join-Path $install 'uninstall.exe'
            if (Test-Path -LiteralPath $uninstaller) {
                Invoke-Installer $uninstaller '/S'
                $until = [DateTime]::UtcNow.AddSeconds(30)
                while ((Test-Path -LiteralPath $uninstaller) -and [DateTime]::UtcNow -lt $until) { Start-Sleep -Milliseconds 200 }
                if ((Test-Path -LiteralPath $uninstaller) -or (Test-Path -LiteralPath (Join-Path $install 'xyzaishell.exe'))) {
                    throw 'Final uninstaller did not remove the installed application'
                }
                if ($null -ne $finalFingerprint) { Assert-ConfigFingerprint $finalFingerprint }
            }
        } finally {
            if (Test-Path -LiteralPath $fixture) { Remove-Item -LiteralPath $fixture -Recurse -Force -ErrorAction Stop }
        }
        if (Test-Path -LiteralPath $fixture) { throw 'Disposable native fixture cleanup failed' }
        Write-Output 'PASS: disposable application data and install fixture removed'
    } finally {
        Stop-Transcript | Out-Null
    }
}
