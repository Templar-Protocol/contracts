//! The one pinned Soroban release. Swapping the release the CLI deploys is a
//! change to this file only: replace the tag and the seven table rows.
//!
//! Every consumer (deploy, plan, doctor, `governance submit-upgrade`, the live
//! release test, `tests/container_smoke.sh`) reads this data; nothing else
//! carries a release tag, byte length or digest.

/// GitHub release tag whose assets the CLI pins.
pub const RELEASE_TAG: &str = "soroban-v1.3.0-rc.1";

/// `(asset file name, exact byte length, lowercase SHA-256 hex)` for every
/// asset of [`RELEASE_TAG`]. One row per line so shell tooling can parse it.
#[rustfmt::skip]
pub const RELEASE_ASSETS: [(&str, usize, &str); 7] = [
    ("templar_soroban_runtime.wasm", 126_503, "5b666c7503faf0f05978677b9b4b8810fb30cf8bcb5610a3e7abc750dc58a286"),
    ("templar_soroban_governance.wasm", 61_735, "efaba9c3bc86ba28acdb7c5c2c7f71dae118b62e6613717ea7e180c7566c1735"),
    ("templar_soroban_share_token.wasm", 11_738, "45fc5d67a165af37334f7a94cf36d2a9eac70d32a7f1fa4f61810a1bb7ccd50b"),
    ("templar_4626_proxy_soroban.wasm", 24_046, "ea91d140eb410ca76921cd5fa3e9f5d09c46aa9e4d400d5142e22fae49649a7d"),
    ("templar_curator_proxy_soroban.wasm", 50_558, "468b2de9f75a781246f00e139c80989364d79223760e3011f27cec858439d192"),
    ("templar_soroban_blend_adapter.wasm", 12_489, "5e20fd03ab74f0770b0d6c58f4d50a4f25a8806878b38fd761444482cac7e95d"),
    ("templar_soroban_custodial_adapter.wasm", 11_900, "cb7c7b6e02a5f7d6b4b6a6151d190939733927950fc46086d1a4107a393222e0"),
];
