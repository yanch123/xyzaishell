# Official Microsoft download, pinned to the installed WebView2 runtime version.
# Never substitute an unmatched Edge browser version or disable signature checks.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:CI -ne 'true' -or $env:RUNNER_OS -ne 'Windows') { throw 'Disposable Windows CI only' }
# Read only the installed runtime executable in the Windows known folder;
# app tests replace LOCALAPPDATA with a disposable fixture, which must not hide
# an already-installed per-user runtime. No browser profile/keyring is read.
$userRuntimeRoot = [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
$runtimes = @(
    Get-ChildItem "${env:ProgramFiles(x86)}\Microsoft\EdgeWebView\Application\*\msedgewebview2.exe" -ErrorAction SilentlyContinue
    Get-ChildItem "$env:ProgramFiles\Microsoft\EdgeWebView\Application\*\msedgewebview2.exe" -ErrorAction SilentlyContinue
    Get-ChildItem "$env:LOCALAPPDATA\Microsoft\EdgeWebView\Application\*\msedgewebview2.exe" -ErrorAction SilentlyContinue
    Get-ChildItem "$userRuntimeRoot\Microsoft\EdgeWebView\Application\*\msedgewebview2.exe" -ErrorAction SilentlyContinue
) | Sort-Object { [version]$_.VersionInfo.ProductVersion } -Descending
if (-not $runtimes) { throw 'No installed WebView2 runtime found on the disposable runner' }
$runtime = $runtimes[0]
$version = $runtime.VersionInfo.ProductVersion
if ($version -notmatch '^\d+\.\d+\.\d+\.\d+$') { throw 'Unexpected WebView2 runtime version' }
$directory = Join-Path $env:RUNNER_TEMP 'xyz-edge-driver'
New-Item -ItemType Directory -Path $directory | Out-Null
$archive = Join-Path $directory 'edgedriver_win64.zip'
$url = "https://msedgedriver.microsoft.com/$version/edgedriver_win64.zip"
Invoke-WebRequest -Uri $url -OutFile $archive
Expand-Archive -LiteralPath $archive -DestinationPath $directory
$driver = Join-Path $directory 'msedgedriver.exe'
$signature = Get-AuthenticodeSignature -LiteralPath $driver
if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation') {
    throw 'Microsoft Edge Driver Authenticode verification failed'
}
$reported = & $driver --version
if ($LASTEXITCODE -ne 0 -or $reported -notmatch ('\b' + [regex]::Escape($version) + '\b')) {
    throw 'Edge Driver does not match the installed WebView2 runtime'
}
# Supported process-only WebView2 overrides make the tested runtime unambiguous
# and keep all browser data inside the wrapper's private fixture.
$env:WEBVIEW2_BROWSER_EXECUTABLE_FOLDER = $runtime.DirectoryName
$env:WEBVIEW2_USER_DATA_FOLDER = Join-Path $env:LOCALAPPDATA 'webview2-user-data'
$evidence = Join-Path $env:RUNNER_TEMP 'xyzaishell-windows-smoke'
New-Item -ItemType Directory -Path $evidence -Force | Out-Null
@{
    runtimeVersion = $version
    runtimePath = $runtime.FullName
    runtimeSelection = 'Process-local WEBVIEW2_BROWSER_EXECUTABLE_FOLDER pins this installed runtime for the test'
    userDataFolder = $env:WEBVIEW2_USER_DATA_FOLDER
    driverVersion = "$reported"
    downloadUrl = $url
    archiveSha256 = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash
    driverSha256 = (Get-FileHash -LiteralPath $driver -Algorithm SHA256).Hash
    signature = "$($signature.Status)"
    signer = $signature.SignerCertificate.Subject
    limitation = 'SHA256 records downloaded bytes; authenticity is checked using Microsoft Authenticode, not a vendor checksum manifest.'
} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $evidence 'driver-provenance.json') -Encoding utf8
"NATIVE_DRIVER=$driver" | Out-File -FilePath $env:GITHUB_ENV -Append -Encoding utf8
$env:NATIVE_DRIVER = $driver
