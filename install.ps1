[CmdletBinding()]
param(
    [string]$Version = "latest",
    [string]$Prefix = (Join-Path $env:LOCALAPPDATA "YAM"),
    [switch]$NoStart
)

$ErrorActionPreference = "Stop"
$repo = "nyakokitsu/yam"
$headers = @{ "User-Agent" = "YAM-installer"; "Accept" = "application/vnd.github+json" }

if ($Version -eq "latest") {
    $release = Invoke-RestMethod -Headers $headers -Uri "https://api.github.com/repos/$repo/releases/latest"
    $tag = $release.tag_name
} else {
    $tag = if ($Version.StartsWith("v")) { $Version } else { "v$Version" }
}

$baseUrl = "https://github.com/$repo/releases/download/$tag"
$binDir = Join-Path $Prefix "bin"
$dataDir = Join-Path $Prefix "data"
$yamd = Join-Path $binDir "yamd.exe"
$yam = Join-Path $binDir "yam.exe"
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("yam-" + [guid]::NewGuid())

function Get-ReleaseAsset([string]$Name) {
    $path = Join-Path $tmp $Name
    Invoke-WebRequest -Headers $headers -Uri "$baseUrl/$Name" -OutFile $path
    return $path
}

try {
    New-Item -ItemType Directory -Force -Path $tmp, $binDir, $dataDir | Out-Null
    $checksums = Get-ReleaseAsset "SHA256SUMS"
    foreach ($name in @("yamd-x86_64-pc-windows-msvc.exe", "yam-x86_64-pc-windows-msvc.exe")) {
        $download = Get-ReleaseAsset $name
        $expected = ((Get-Content $checksums | Where-Object { $_ -match ("\\s" + [regex]::Escape($name) + "$") }) -split "\\s+")[0]
        if (-not $expected) { throw "No SHA-256 checksum for $name" }
        $actual = (Get-FileHash -Algorithm SHA256 $download).Hash.ToLowerInvariant()
        if ($actual -ne $expected.ToLowerInvariant()) { throw "Checksum mismatch for $name" }
        Copy-Item -Force $download (Join-Path $binDir ($name -replace "-x86_64-pc-windows-msvc", ""))
    }
} finally {
    if (Test-Path $tmp) { Remove-Item -Force -Recurse $tmp }
}

$runValue = ('"{0}" --data-dir "{1}"' -f $yamd, $dataDir)
Set-ItemProperty -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" -Name "YAM" -Value $runValue

if (-not $NoStart) {
    Start-Process -FilePath $yamd -ArgumentList @("--data-dir", $dataDir) | Out-Null
}

Write-Host "YAM $tag installed in $Prefix"
Write-Host "Open http://127.0.0.1:8099"
