$ErrorActionPreference = 'Stop'

$BinName = 'e-sh'
$Version = (Select-String -Path 'Cargo.toml' -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
$Root = Resolve-Path "$PSScriptRoot\.."
$Dist = Join-Path $Root 'dist'
New-Item -ItemType Directory -Force -Path $Dist | Out-Null

Write-Host ">>> Windows x86_64 ($BinName $Version)"
rustup target add x86_64-pc-windows-msvc | Out-Null
cargo build --release --target x86_64-pc-windows-msvc
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$StageName = "$BinName-$Version-windows-x86_64"
$Stage = Join-Path $Dist $StageName
if (Test-Path $Stage) { Remove-Item -Recurse -Force $Stage }
New-Item -ItemType Directory -Force -Path $Stage | Out-Null

Copy-Item "target\x86_64-pc-windows-msvc\release\$BinName.exe" $Stage
if (Test-Path 'README.md') { Copy-Item 'README.md' $Stage }

$Zip = Join-Path $Dist "$StageName.zip"
if (Test-Path $Zip) { Remove-Item -Force $Zip }
Compress-Archive -Path $Stage -DestinationPath $Zip
Remove-Item -Recurse -Force $Stage

$Hash = Get-FileHash -Algorithm SHA256 $Zip
"$($Hash.Hash.ToLower())  $StageName.zip" | Out-File -Encoding ascii "$Zip.sha256"

Write-Host ""
Write-Host "Done. Artifacts in: $Dist"
Get-ChildItem $Dist
