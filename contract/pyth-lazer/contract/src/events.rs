use near_sdk::{json_types::Base64VecU8, near, AccountId, NearToken};
use templar_common::oracle::lazer::FeedData;
use templar_common::upgrade::UpgradeSummary;

use crate::Config;

/// NEP-297 events emitted by the adapter. Every owner-gated `admin_*` mutation emits one, carrying
/// the resulting value (not an old→new diff), so configuration and governance changes leave an
/// on-chain audit trail. Ownership transfer/renounce is not covered here — `near_sdk_contract_tools`
/// `Owner` already emits its own NEP-297 events under the `x-own` standard.
#[near(event_json(standard = "pyth-lazer-adapter"))]
pub enum PythLazerEvent {
    /// Emitted after a successful `update_price_feeds`, listing the feeds that were written
    /// (by Lazer feed id) together with their new data.
    #[event_version("1.0.0")]
    UpdatePrices { updated_feeds: Vec<(u32, FeedData)> },

    /// Emitted after `admin_set_config` stores a new validated `Config`.
    #[event_version("1.0.0")]
    ConfigSet { config: Config },

    /// Emitted after `admin_set_signer` adds or refreshes a signer (`expires_at_s = Some`).
    #[event_version("1.0.0")]
    SignerUpserted {
        public_key: String,
        expires_at_s: u64,
    },

    /// Emitted after `admin_set_signer` removes a signer (`expires_at_s = None`).
    #[event_version("1.0.0")]
    SignerRemoved { public_key: String },

    /// Emitted after `admin_withdraw` schedules a transfer to the owner.
    #[event_version("1.0.0")]
    Withdrawn {
        amount: NearToken,
        receiver: AccountId,
    },

    /// Emitted by `admin_upgrade` before it returns the atomic deploy+migrate `Promise`. `code`
    /// summarizes the new code (a blob's sha256, or a global-contract reference — never the blob
    /// itself). `migrate_args` is the migration selector that ran, or `None` for a same-version
    /// refresh (no migration).
    #[event_version("2.0.0")]
    Upgraded {
        code: UpgradeSummary,
        migrate_args: Option<Base64VecU8>,
    },
}
