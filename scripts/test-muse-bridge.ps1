#requires -Version 5.1
<#
.SYNOPSIS
  Exercise the Muse bridge's auth, project-root confinement and removed routes.

.DESCRIPTION
  Starts an isolated `Muse.exe --headless` on a free loopback port with a temporary data folder,
  a temporary project root and a deliberately unique instance name, so it never touches the
  bridge that may already be serving this PC.

  The suite proves:
    * /health is unauthenticated, every other route refuses a missing or wrong token;
    * reads, writes, trash, zip and unzip stay inside the project root;
    * a junction and a zip entry that escape the root are refused, and nothing is written through
      them;
    * /files/open and /files/delete stay blocked, even with RIGEL_MUSE_FULL_CONTROL=1 set;
    * exec, apps, screen, UI, Resolve and Express routes are gone (403/404, never 200).

  Exit code 0 means every check passed. Run from anywhere:
      powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-muse-bridge.ps1
#>
[CmdletBinding()]
param(
    [string]$MuseExe = '',
    [int]$Port = 0,
    [int]$TimeoutSeconds = 20
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
if ([string]::IsNullOrWhiteSpace($MuseExe)) {
    $MuseExe = Join-Path $repo 'vendor\muse-desktop\dist\Muse.exe'
}
if (-not (Test-Path -LiteralPath $MuseExe)) {
    throw "Muse.exe not found at $MuseExe. Build it with:`n  vendor\muse-desktop\.dotnet\dotnet.exe publish vendor\muse-desktop\MuseDesktop.csproj -c Release -r win-x64 --self-contained true -o vendor\muse-desktop\dist"
}

function Get-FreePort {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $listener.Start()
    $chosen = ([System.Net.IPEndPoint]$listener.LocalEndpoint).Port
    $listener.Stop()
    return $chosen
}

if ($Port -le 0) { $Port = Get-FreePort }
$base = Join-Path $env:TEMP ('rigel-muse-bridge-test-' + [guid]::NewGuid().ToString('N'))
$root = Join-Path $base 'root'
$outside = Join-Path $base 'outside'
$data = Join-Path $base 'data'
$junction = Join-Path $root 'escape-junction'
$zipSlip = Join-Path $root 'slip.zip'
$zipBundle = Join-Path $root 'bundle'
$zipFile = Join-Path $root 'bundle.zip'
$zipOut = Join-Path $root 'unzipped'
$process = $null
$failures = 0
$checks = 0

function Write-Result([string]$name, [bool]$passed, [string]$detail = '') {
    $script:checks++
    if ($passed) {
        Write-Host ("PASS  " + $name)
    } else {
        $script:failures++
        $message = "FAIL  $name"
        if ($detail) { $message += " - $detail" }
        Write-Host $message -ForegroundColor Red
    }
}

function Get-Response {
    param(
        [string]$Method = 'POST',
        [string]$Path,
        [object]$Body = $null,
        [string]$Token = '',
        [switch]$Anonymous
    )
    $headers = @{}
    if (-not $Anonymous -and $Token) { $headers['Authorization'] = "Bearer $Token" }
    $uri = "http://127.0.0.1:$Port$Path"
    $parameters = @{
        Uri = $uri
        Method = $Method
        Headers = $headers
        UseBasicParsing = $true
        TimeoutSec = 10
    }
    if ($null -ne $Body) {
        $parameters['Body'] = ($Body | ConvertTo-Json -Depth 10 -Compress)
        $parameters['ContentType'] = 'application/json'
    }
    try {
        $response = Invoke-WebRequest @parameters
        $text = [string]$response.Content
        return [pscustomobject]@{
            Status = [int]$response.StatusCode
            Text = $text
            Json = if ($text) { $text | ConvertFrom-Json } else { $null }
        }
    } catch [System.Net.WebException] {
        $response = $_.Exception.Response
        if ($null -eq $response) { throw }
        $reader = New-Object System.IO.StreamReader($response.GetResponseStream())
        try { $text = $reader.ReadToEnd() } finally { $reader.Dispose() }
        return [pscustomobject]@{
            Status = [int]$response.StatusCode
            Text = $text
            Json = if ($text) { $text | ConvertFrom-Json } else { $null }
        }
    }
}

function Test-Status([string]$name, $response, [int]$expected) {
    Write-Result $name ($response.Status -eq $expected) "status $($response.Status), expected ${expected}: $($response.Text)"
}

try {
    New-Item -ItemType Directory -Force -Path $root, $outside, $data | Out-Null
    Set-Content -LiteralPath (Join-Path $root 'inside.txt') -Value 'inside' -NoNewline
    Set-Content -LiteralPath (Join-Path $outside 'outside.txt') -Value 'outside' -NoNewline
    Set-Content -LiteralPath (Join-Path $root 'trash-me.txt') -Value 'trash' -NoNewline

    # A real junction is the escape a string-only prefix check misses.
    New-Item -ItemType Junction -Path $junction -Target $outside | Out-Null

    # A zip entry with traversal. The bridge must reject the archive before extraction.
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [System.IO.Compression.ZipFile]::Open($zipSlip, 'Create')
    try {
        $entry = $archive.CreateEntry('../zip-escape.txt')
        $writer = New-Object System.IO.StreamWriter($entry.Open())
        try { $writer.Write('escaped') } finally { $writer.Dispose() }
    } finally {
        $archive.Dispose()
    }

    New-Item -ItemType Directory -Force -Path $zipBundle | Out-Null
    Set-Content -LiteralPath (Join-Path $zipBundle 'nested.txt') -Value 'nested' -NoNewline

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = (Resolve-Path $MuseExe).Path
    $psi.Arguments = '--headless'
    $psi.WorkingDirectory = Split-Path -Parent $MuseExe
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.EnvironmentVariables['RIGEL_MUSE_DATA'] = $data
    $psi.EnvironmentVariables['RIGEL_MUSE_ROOT'] = $root
    $psi.EnvironmentVariables['RIGEL_MUSE_PORT'] = [string]$Port
    $psi.EnvironmentVariables['RIGEL_MUSE_INSTANCE'] = [guid]::NewGuid().ToString('N')
    # The old escape hatch must not reopen anything. Set it deliberately.
    $psi.EnvironmentVariables['RIGEL_MUSE_FULL_CONTROL'] = '1'
    $process = [System.Diagnostics.Process]::Start($psi)

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    $health = $null
    while ((Get-Date) -lt $deadline) {
        if ($process.HasExited) {
            throw "Muse bridge exited before it answered /health (exit $($process.ExitCode))."
        }
        try {
            $health = Get-Response -Method GET -Path '/health' -Anonymous
            if ($health.Status -eq 200) { break }
        } catch { }
        Start-Sleep -Milliseconds 250
    }
    if ($null -eq $health -or $health.Status -ne 200) {
        throw "Muse bridge did not answer /health on port $Port within $TimeoutSeconds seconds."
    }

    Test-Status 'health is unauthenticated' $health 200
    Test-Status 'info refuses no token' (Get-Response -Method GET -Path '/info' -Anonymous) 401
    Test-Status 'info refuses the wrong token' (Get-Response -Method GET -Path '/info' -Token 'not-the-token') 401

    $tokenPath = Join-Path $data 'bridge-token.txt'
    if (-not (Test-Path -LiteralPath $tokenPath)) { throw "bridge token missing at $tokenPath" }
    $token = (Get-Content -LiteralPath $tokenPath -Raw).Trim()

    $info = Get-Response -Method GET -Path '/info' -Token $token
    Test-Status 'info accepts the bridge token' $info 200
    Write-Result 'info reports the configured project root' (
        $info.Json.ok -and ([string]$info.Json.data.fileRoot).TrimEnd('\') -eq $root.TrimEnd('\')
    ) ([string]$info.Json.data.fileRoot)
    Write-Result 'info no longer advertises full control' (
        $null -eq $info.Json.data.PSObject.Properties['fullControl']
    )

    Test-Status 'read inside the root' (
        Get-Response -Path '/files/read-text' -Token $token -Body @{ path = 'inside.txt' }
    ) 200

    $absoluteEscape = Get-Response -Path '/files/read-text' -Token $token -Body @{ path = (Join-Path $outside 'outside.txt') }
    Test-Status 'absolute path outside the root' $absoluteEscape 403
    Test-Status 'dot-dot path outside the root' (
        Get-Response -Path '/files/read-text' -Token $token -Body @{ path = '..\outside\outside.txt' }
    ) 403
    Test-Status 'read through a junction' (
        Get-Response -Path '/files/read-text' -Token $token -Body @{ path = 'escape-junction\outside.txt' }
    ) 403
    Test-Status 'write through a junction' (
        Get-Response -Path '/files/write-text' -Token $token -Body @{ path = 'escape-junction/planted.txt'; text = 'no' }
    ) 403
    Write-Result 'junction write left no file outside' (-not (Test-Path -LiteralPath (Join-Path $outside 'planted.txt')))

    Test-Status 'trash outside the root' (
        Get-Response -Path '/files/trash' -Token $token -Body @{ path = (Join-Path $outside 'outside.txt') }
    ) 403
    Write-Result 'trash outside left the file in place' (Test-Path -LiteralPath (Join-Path $outside 'outside.txt'))

    Test-Status 'zip through a junction' (
        Get-Response -Path '/files/zip' -Token $token -Body @{ from = 'escape-junction'; to = 'junction.zip' }
    ) 403

    $slip = Get-Response -Path '/files/unzip' -Token $token -Body @{ from = 'slip.zip'; to = 'unzipped' }
    Test-Status 'zip-slip archive is refused' $slip 403
    Write-Result 'zip-slip wrote nothing beside the root' (-not (Test-Path -LiteralPath (Join-Path $root '..\zip-escape.txt')))

    $zip = Get-Response -Path '/files/zip' -Token $token -Body @{ from = 'bundle'; to = 'bundle.zip' }
    Test-Status 'zip inside the root' $zip 200
    $unzip = Get-Response -Path '/files/unzip' -Token $token -Body @{ from = 'bundle.zip'; to = 'unzipped' }
    Test-Status 'unzip inside the root' $unzip 200
    Write-Result 'zip round trip kept its content' (
        Test-Path -LiteralPath (Join-Path $root 'unzipped\bundle\nested.txt')
    )

    Test-Status '/files/open is permanently blocked' (
        Get-Response -Path '/files/open' -Token $token -Body @{ path = 'inside.txt' }
    ) 403
    Test-Status '/files/delete is permanently blocked' (
        Get-Response -Path '/files/delete' -Token $token -Body @{ path = 'trash-me.txt'; confirm = $true }
    ) 403
    Write-Result '/files/delete left the file in place' (Test-Path -LiteralPath (Join-Path $root 'trash-me.txt'))

    foreach ($removed in @('/exec/run', '/apps/list', '/screen/screenshot', '/ui/windows', '/resolve/detect', '/express/detect')) {
        $response = Get-Response -Path $removed -Token $token -Body @{ command = 'whoami'; action = 'info' }
        Write-Result "$removed is removed" ($response.Status -in @(403, 404)) "status $($response.Status): $($response.Text)"
    }

    Write-Host ''
    if ($failures -eq 0) {
        Write-Host "Muse bridge security: $checks checks passed." -ForegroundColor Green
        exit 0
    }
    Write-Host "Muse bridge security: $failures of $checks checks FAILED." -ForegroundColor Red
    exit 1
} finally {
    if ($null -ne $process -and -not $process.HasExited) {
        try { Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue } catch { }
    }
    # Remove the junction before its target, so a recursive delete cannot wander outside.
    if (Test-Path -LiteralPath $junction) {
        try { & cmd.exe /c rmdir "$junction" | Out-Null } catch { }
    }
    if (Test-Path -LiteralPath $base) {
        try { Remove-Item -LiteralPath $base -Recurse -Force -ErrorAction SilentlyContinue } catch { }
    }
}
