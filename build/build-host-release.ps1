<#
    OpenSSL is already installed on windows-latest virtual environment.
    If you need OpenSSL, consider install it by:

    choco install openssl
#>
choco install openssl --confirm
$ErrorActionPreference = "Stop"

$TargetTriple = (rustc -Vv | Select-String -Pattern "host: (.*)" | ForEach-Object { $_.Matches.Value }).split()[-1]

Write-Host "Started building release for ${TargetTriple} ..."

cargo build --release
if (!$?) {
    exit $LASTEXITCODE
}

$Version = (Select-String -Pattern '^version *= *"([^"]*)"$' -Path "${PSScriptRoot}\..\Cargo.toml" | ForEach-Object { $_.Matches.Value }).split()[-1]
$Version = $Version -replace '"'

$PackageReleasePath = "${PSScriptRoot}\release"
$PackageName = "ansible-rs${Version}.${TargetTriple}.zip"
$PackagePath = "${PackageReleasePath}\${PackageName}"

Write-Host $Version
Write-Host $PackageReleasePath
Write-Host $PackageName
Write-Host $PackagePath

Push-Location "${PSScriptRoot}\..\target\release"

$ProgressPreference = "SilentlyContinue"
New-Item "${PackageReleasePath}" -ItemType Directory -ErrorAction SilentlyContinue
$CompressParam = @{
    LiteralPath     = "ansible-rs"
    DestinationPath = "${PackagePath}"
}
Compress-Archive @CompressParam

Write-Host "Created release packet ${PackagePath}"

$PackageChecksumPath = "${PackagePath}.sha256"
$PackageHash = (Get-FileHash -Path "${PackagePath}" -Algorithm SHA256).Hash
"${PackageHash}  ${PackageName}" | Out-File -FilePath "${PackageChecksumPath}"

Write-Host "Created release packet checksum ${PackageChecksumPath}"
