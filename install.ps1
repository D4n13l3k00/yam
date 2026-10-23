[CmdletBinding()]
param(
    [string]$Version = "latest",
    [string]$Prefix = (Join-Path $env:LOCALAPPDATA "YAM")
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
$yam = Join-Path $binDir "yam.exe"
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("yam-" + [guid]::NewGuid())

function Get-ReleaseAsset([string]$Name) {
    $path = Join-Path $tmp $Name
    Invoke-WebRequest -Headers $headers -Uri "$baseUrl/$Name" -OutFile $path
    return $path
}

try {
    New-Item -ItemType Directory -Force -Path $tmp, $binDir | Out-Null
    $checksums = Get-ReleaseAsset "SHA256SUMS"
    foreach ($name in @("yam-x86_64-pc-windows-msvc.exe")) {
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

Write-Host "YAM $tag installed in $Prefix"
Write-Host "Use yam --url http://<speaker-ip>:8099 ..."
