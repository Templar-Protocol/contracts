//! The one pinned Soroban release. Swapping the release the CLI deploys is a
//! change to this file only: replace the tag and the seven table rows.
//!
//! Every consumer (deploy, plan, doctor, `governance submit-upgrade`, the live
//! release test, `tests/container_smoke.sh`) reads this data; nothing else
//! carries a release tag, byte length or digest.

/// GitHub release tag whose assets the CLI pins.
pub const RELEASE_TAG: &str = "soroban-v1.1.1";

/// `(asset file name, exact byte length, lowercase SHA-256 hex)` for every
/// asset of [`RELEASE_TAG`]. One row per line so shell tooling can parse it.
#[rustfmt::skip]
pub const RELEASE_ASSETS: [(&str, usize, &str); 7] = [
    ("templar_soroban_runtime.wasm", 129_499, "4d24790f3ea2a02e521b84d583dab00bfa246cdfd06ee858f1f656a831cccc83"),
    ("templar_soroban_governance.wasm", 61_598, "7e9b4247082227ddbf8567eb080c1d9c2ff181058e27b1effff60f2513b78b2e"),
    ("templar_soroban_share_token.wasm", 11_816, "b0427e26c0b5201be42b2d0577773081b2f5a382507995e1ad9ca7f0ee241da5"),
    ("templar_4626_proxy_soroban.wasm", 24_142, "b71d716ddda2b6648ece8ea8cacb3abfd0a893d3dc43e44b495c9df072e4ae9d"),
    ("templar_curator_proxy_soroban.wasm", 50_250, "a2932a58e22d206bd63732d7023321bbc9bb1c629865b4767e1ae589228f7868"),
    ("templar_soroban_blend_adapter.wasm", 12_112, "ce0024538e49cb726c861bee210f83bebca0a2352fec79f9dbdc25142df647cf"),
    ("templar_soroban_custodial_adapter.wasm", 11_981, "03c1c749613d67d5ed460bc4fbb12eedb1a043068665a295fbefb774588a796b"),
];
