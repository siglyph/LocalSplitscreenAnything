# Builds lsa.exe, both hook DLLs and the 32-bit injector helper, and collects everything
# into dist\.
# Needs: Rust stable (MSVC) with x86_64-pc-windows-msvc and i686-pc-windows-msvc targets,
# Visual Studio 2022 Build Tools (C++ workload; MinHook is compiled from C).
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

cargo build --release -p lsa-cli
cargo build --profile hook -p lsa-hook --target x86_64-pc-windows-msvc
cargo build --profile hook -p lsa-hook --target i686-pc-windows-msvc
# The 64-bit orchestrator cannot inject across bitness, so the 32-bit games are injected by
# this tiny i686 helper.
cargo build --release -p lsa-inject32 --target i686-pc-windows-msvc

New-Item -ItemType Directory -Force -Path dist | Out-Null
Copy-Item -Force target\release\lsa.exe dist\
Copy-Item -Force target\x86_64-pc-windows-msvc\hook\lsa_hook.dll dist\lsa_hook64.dll
Copy-Item -Force target\i686-pc-windows-msvc\hook\lsa_hook.dll dist\lsa_hook32.dll
Copy-Item -Force target\i686-pc-windows-msvc\release\lsa-inject32.exe dist\
Copy-Item -Recurse -Force profiles dist\

Write-Host "Built dist\lsa.exe, dist\lsa_hook64.dll, dist\lsa_hook32.dll, dist\lsa-inject32.exe"
Write-Host "Try: dist\lsa.exe monitors"
