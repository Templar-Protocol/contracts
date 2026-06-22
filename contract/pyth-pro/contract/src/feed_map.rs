//! Feed-id mapping layer — the ONLY coupling between Pyth's 32-byte `PriceIdentifier` space and
//! Lazer's `u32` feed-id space.
//!
//! Everything else in the contract (storage, the write path) is keyed by `u32`. If this mapping
//! is later relocated to the proxy-oracle, deleting this module and the `Contract::ids` field —
//! then exposing feed-id-keyed views — is all that's required; nothing else depends on it.

use near_sdk::{assert_one_yocto, near};
use near_sdk_contract_tools::owner::Owner;
use templar_common::{contract::list, oracle::pyth::PriceIdentifier};

use crate::{Contract, ContractExt};

impl Contract {
    /// Translate a consumer-facing price identifier into a Lazer feed id, if mapped.
    pub(crate) fn resolve(&self, price_identifier: &PriceIdentifier) -> Option<u32> {
        self.ids.get(price_identifier).copied()
    }
}

#[near]
impl Contract {
    /// Map (`feed_id = Some`) or unmap (`feed_id = None`) a consumer `PriceIdentifier` to a Lazer
    /// feed id. Map to the asset's existing Pyth Core identifier to make this a drop-in oracle.
    ///
    /// Unmapping removes only the `PriceIdentifier -> feed_id` entry; the stored `feeds[feed_id]`
    /// data is intentionally retained (it is global, keyed by the Lazer id, and may be referenced
    /// by other identifiers). Consequently, re-mapping a `PriceIdentifier` to a feed id that still
    /// holds data makes `price_feed_exists` true and lets the `*_unsafe` views return that
    /// pre-existing data immediately, before any fresh update. The age-gated `*_no_older_than`
    /// views (which Templar consumers use) filter such stale data by `publish_time`, so this is an
    /// operational caveat for `*_unsafe`/`price_feed_exists` callers during onboarding or feed
    /// replacement — re-push an update (or query with a freshness bound) after (re)mapping.
    #[payable]
    pub fn admin_set_feed_mapping(
        &mut self,
        price_identifier: PriceIdentifier,
        feed_id: Option<u32>,
    ) {
        assert_one_yocto();
        Self::require_owner();
        match feed_id {
            Some(id) => {
                self.ids.insert(price_identifier, id);
            }
            None => {
                self.ids.remove(&price_identifier);
            }
        }
    }

    /// The Lazer feed id a `PriceIdentifier` resolves to, if any.
    pub fn get_feed_mapping(&self, price_identifier: PriceIdentifier) -> Option<u32> {
        self.resolve(&price_identifier)
    }

    /// Paginated list of all configured `(PriceIdentifier, feed_id)` mappings.
    pub fn list_feed_mappings(
        &self,
        offset: Option<u32>,
        count: Option<u32>,
    ) -> Vec<(PriceIdentifier, u32)> {
        list(
            self.ids.iter().map(|(id, feed_id)| (*id, *feed_id)),
            offset,
            count,
        )
    }
}
