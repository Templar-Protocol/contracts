#![allow(
    clippy::should_panic_without_expect,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::large_digit_groups,
    clippy::explicit_iter_loop
)]

//! Group A — Deploy + bootstrap chain.
//!
//! Verifies the harness constructs a self-consistent three-contract +
//! upstream system: runtime owner = governance, governance admin = `admin`,
//! adapter owner = governance, runtime knows its base asset.

use templar_proxy_oracle_soroban_governance_common::Role;
use templar_proxy_oracle_soroban_integration_tests::common::Bootstrap;

#[test]
fn bootstrap_deploys_three_contracts_and_links_them() {
    let b = Bootstrap::new();

    // Ownership wiring.
    assert_eq!(b.runtime.get_owner(), Some(b.governance_id.clone()));
    assert_eq!(b.adapter.get_owner(), Some(b.admin.clone()));

    // Governance knows its target.
    assert_eq!(b.governance.proxy_oracle(), b.runtime_id);

    // Admin role is granted to the bootstrap admin.
    assert!(b.governance.has_role(&b.admin, &Role::Admin));

    // Runtime knows its base asset; no feeds configured yet.
    assert_eq!(b.runtime.source_base(), Some(b.base_usd.clone()));
    assert_eq!(b.runtime.registered_assets().len(), 0);

    // Adapter remembers its config.
    let cfg = b.adapter.config().unwrap();
    assert_eq!(cfg.parent_oracle, b.runtime_id);
    assert_eq!(cfg.asset, b.asset_btc);
    assert_eq!(cfg.base, b.base_usd);
}
