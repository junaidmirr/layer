# Layer Windows Installer
# Usage: irm https://raw.githubusercontent.com/junaidmirr/layer/master/install.ps1 | iex

$ErrorActionPreference = "Stop"

$repo = "junaidmirr/layer"
$installDir = Join-Path $HOME ".layer\bin"
$exePath = Join-Path $installDir "layer.exe"

Write-Host "Installing Layer..." -ForegroundColor Cyan

# Fetch latest release data
$releasesUrl = "https://api.github.com/repos/$repo/releases"
try {
    $releases = Invoke-RestMethod -Uri $releasesUrl -Headers @{ "User-Agent" = "layer-installer" }
} catch {
    Write-Error "Failed to fetch release information from GitHub: $_"
    exit 1
}

$release = $releases | Select-Object -First 1
if (-not $release) {
    Write-Error "No releases found for $repo."
    exit 1
}

$tag = $release.tag_name
Write-Host "Selected version: $tag" -ForegroundColor Gray

# Locate the Windows zip asset
$asset = $release.assets | Where-Object { $_.name -like "*x86_64-pc-windows-msvc.zip" } | Select-Object -First 1
if (-not $asset) {
    # Fallback to direct download URL
    $downloadUrl = "https://github.com/$repo/releases/download/$tag/layer-x86_64-pc-windows-msvc.zip"
} else {
    $downloadUrl = $asset.browser_download_url
}

$tempZip = Join-Path $env:TEMP "layer-$tag.zip"
$tempExtract = Join-Path $env:TEMP "layer-extract-$tag"

Write-Host "Downloading $downloadUrl..." -ForegroundColor Gray
Invoke-WebRequest -Uri $downloadUrl -OutFile $tempZip -UseBasicParsing

if (-not (Test-Path $installDir)) {
    New-Item -ItemType Directory -Path $installDir -Force | Out-Null
}

Write-Host "Extracting..." -ForegroundColor Gray
Expand-Archive -Path $tempZip -DestinationPath $tempExtract -Force

$extractedExe = Get-ChildItem -Path $tempExtract -Filter "layer.exe" -Recurse | Select-Object -First 1
if (-not $extractedExe) {
    Write-Error "Could not find layer.exe inside the downloaded archive."
    exit 1
}

Copy-Item -Path $extractedExe.FullName -Destination $exePath -Force

# Clean up temporary files
Remove-Item -Path $tempZip -Force -ErrorAction SilentlyContinue
Remove-Item -Path $tempExtract -Recurse -Force -ErrorAction SilentlyContinue

# Ensure directory is on User PATH
$userPath = [Environment]::GetEnvironmentVariable("Path", [EnvironmentVariableTarget]::User)
if ($userPath -split ';' -notcontains $installDir) {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$installDir", [EnvironmentVariableTarget]::User)
    $env:Path = "$env:Path;$installDir"
    Write-Host "Added $installDir to your user PATH." -ForegroundColor Yellow
}

Write-Host "`nLayer ($tag) installed successfully!" -ForegroundColor Green
Write-Host "Location: $exePath" -ForegroundColor Gray
Write-Host "`nYou can now run:" -ForegroundColor Cyan
Write-Host "  layer --help" -ForegroundColor White
Write-Host "  layer mcp" -ForegroundColor White
