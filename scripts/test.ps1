$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
. "$PSScriptRoot\dev-env.ps1"
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features --locked
cargo deny check
cargo deny --manifest-path fuzz/Cargo.toml --config fuzz/deny.toml --locked check
& "$PSScriptRoot\check-crypto-graph.ps1"
& "$PSScriptRoot\generate-supply-chain-artifacts.ps1"
