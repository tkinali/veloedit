# Builds the release binary and packs it into dist\veloedit-<version>-x86_64-windows.zip
# (veloedit.exe, licenses, README) plus a .sha256 file. Run it on Windows, in PowerShell 7:
#
#   pwsh scripts/package-windows.ps1
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$name = "veloedit-$version-x86_64-windows"

cargo build --release --locked
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$dir = "dist/$name"
Remove-Item -Recurse -Force $dir, "dist/$name.zip", "dist/$name.zip.sha256" -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $dir | Out-Null
Copy-Item target/release/veloedit.exe, LICENSE, THIRD-PARTY-LICENSES.txt, README.md $dir
Compress-Archive -Path $dir -DestinationPath "dist/$name.zip"

$hash = (Get-FileHash "dist/$name.zip" -Algorithm SHA256).Hash.ToLower()
"$hash  $name.zip" | Out-File -FilePath "dist/$name.zip.sha256" -Encoding ascii

$mb = [math]::Round((Get-Item "dist/$name.zip").Length / 1MB, 1)
Write-Host "built dist/$name.zip ($mb MB)"
