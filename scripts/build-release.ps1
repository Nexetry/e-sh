$ErrorActionPreference = 'Stop'

$BinName = 'e-sh'
$RdpBinName = 'e-sh-rdp'
$Version = (Select-String -Path 'Cargo.toml' -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
$Root = Resolve-Path "$PSScriptRoot\.."
$Dist = Join-Path $Root 'dist'
New-Item -ItemType Directory -Force -Path $Dist | Out-Null

Write-Host ">>> Windows x86_64 ($BinName $Version)"
rustup target add x86_64-pc-windows-msvc | Out-Null

# Main binary
cargo build --release --target x86_64-pc-windows-msvc
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

# RDP helper binary
Write-Host "    ... building $RdpBinName"
cargo build --release --target x86_64-pc-windows-msvc --manifest-path "$Root\e-sh-rdp\Cargo.toml"
if ($LASTEXITCODE -ne 0) { throw "cargo build (e-sh-rdp) failed" }

$StageName = "$BinName-$Version-windows-x86_64"
$Stage = Join-Path $Dist $StageName
if (Test-Path $Stage) { Remove-Item -Recurse -Force $Stage }
New-Item -ItemType Directory -Force -Path $Stage | Out-Null

Copy-Item "target\x86_64-pc-windows-msvc\release\$BinName.exe" $Stage
Copy-Item "$Root\e-sh-rdp\target\x86_64-pc-windows-msvc\release\$RdpBinName.exe" $Stage
if (Test-Path 'README.md') { Copy-Item 'README.md' $Stage }

$Zip = Join-Path $Dist "$StageName.zip"
if (Test-Path $Zip) { Remove-Item -Force $Zip }
Compress-Archive -Path $Stage -DestinationPath $Zip
Remove-Item -Recurse -Force $Stage

$Hash = Get-FileHash -Algorithm SHA256 $Zip
"$($Hash.Hash.ToLower())  $StageName.zip" | Out-File -Encoding ascii "$Zip.sha256"

Write-Host ""
Write-Host ">>> Building Windows .msi via cargo-wix"
$CargoWix = Get-Command cargo-wix -ErrorAction SilentlyContinue
if (-not $CargoWix) {
    Write-Host "cargo-wix not installed; skipping .msi (install with: cargo install cargo-wix)"
} else {
    if (-not (Test-Path 'wix\main.wxs')) {
        Write-Host "wix/main.wxs missing; running 'cargo wix init --force'"
        cargo wix init --force
        if ($LASTEXITCODE -ne 0) { throw "cargo wix init failed" }
    }
    cargo wix --no-build --nocapture --target x86_64-pc-windows-msvc --output $Dist
    if ($LASTEXITCODE -ne 0) { throw "cargo wix failed" }
    $Msi = Get-ChildItem -Path $Dist -Filter '*.msi' | Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if ($Msi) {
        $MsiHash = Get-FileHash -Algorithm SHA256 $Msi.FullName
        "$($MsiHash.Hash.ToLower())  $($Msi.Name)" | Out-File -Encoding ascii "$($Msi.FullName).sha256"
        Write-Host "    -> $($Msi.FullName)"
    } else {
        Write-Warning "cargo-wix did not produce a .msi"
    }
}

Write-Host ""
Write-Host "Done. Artifacts in: $Dist"
Get-ChildItem $Dist
