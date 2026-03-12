#![allow(clippy::needless_pass_by_value)]

use std::collections::HashMap;

use near_sdk::{
    assert_one_yocto, env,
    json_types::{Base64VecU8, U64},
    near, AccountId, PanicOnDefault,
};
use near_sdk_contract_tools::{rbac::Rbac, Rbac};
use templar_common::{
    contract::list,
    oracle::{
        redstone::{
            Config, FeedData, FeedId, GetPrices, RedStoneAdapter, RedStoneContractInterface,
            RedStoneEvent, Role, SerializableU256,
        },
        time::Milliseconds,
    },
    UnwrapReject,
};

#[derive(Rbac, PanicOnDefault)]
#[rbac(roles = "Role")]
#[near(contract_state)]
pub struct Contract {
    pub adapter: RedStoneAdapter,
}

#[near]
impl Contract {
    #[init]
    pub fn new(config: Config) -> Self {
        let mut self_ = Self {
            adapter: RedStoneAdapter::new(b"a", config),
        };

        let predecessor = env::predecessor_account_id();

        <Self as Rbac>::add_role(&mut self_, &predecessor, &Role::ModifyRoles);
        <Self as Rbac>::add_role(&mut self_, &predecessor, &Role::TrustedUpdater);

        self_
    }

    pub fn get_config(&self) -> &Config {
        &self.adapter.config
    }

    pub fn has_role(&self, account_id: AccountId, role: Role) -> bool {
        <Self as Rbac>::has_role(&account_id, &role)
    }

    #[payable]
    pub fn set_role(&mut self, account_id: AccountId, role: Role, set: Option<bool>) {
        assert_one_yocto();
        let set = set.unwrap_or(true);
        <Self as Rbac>::require_role(&Role::ModifyRoles);
        if set {
            <Self as Rbac>::add_role(self, &account_id, &role);
        } else {
            <Self as Rbac>::remove_role(self, &account_id, &role);
        }
    }

    pub fn list_role(&self, role: Role, offset: Option<u32>, count: Option<u32>) -> Vec<AccountId> {
        list(<Self as Rbac>::iter_members_of(&role), offset, count)
    }
}

#[near]
impl RedStoneContractInterface for Contract {
    fn unique_signer_threshold(&self) -> U64 {
        U64(u64::from(self.adapter.config.signer_count_threshold))
    }

    fn get_prices(&self, feed_ids: Vec<FeedId>, payload: Base64VecU8) -> GetPrices {
        self.adapter
            .get_prices(&feed_ids, &payload.0, Milliseconds::now())
            .unwrap_or_reject()
    }

    fn read_prices(&self, feed_ids: Vec<FeedId>) -> HashMap<FeedId, SerializableU256> {
        let now = Milliseconds::now();
        feed_ids
            .into_iter()
            .filter_map(|feed_id| {
                let data = self.adapter.feed_data(&feed_id, now)?.ok()?;
                Some((feed_id, data.price))
            })
            .collect::<HashMap<_, _>>()
    }

    fn read_timestamp(&self, feed_id: FeedId) -> Option<Milliseconds> {
        let data = self
            .adapter
            .feed_data(&feed_id, Milliseconds::now())?
            .unwrap_or_reject();
        Some(data.package_timestamp)
    }

    fn read_price_data_for_feed(&self, feed_id: FeedId) -> Option<FeedData> {
        let data = self
            .adapter
            .feed_data(&feed_id, Milliseconds::now())?
            .unwrap_or_reject();
        Some(data.clone())
    }

    fn read_price_data(&self, feed_ids: Vec<FeedId>) -> HashMap<FeedId, FeedData> {
        let now = Milliseconds::now();
        feed_ids
            .into_iter()
            .filter_map(|feed_id| {
                let data = self.adapter.feed_data(&feed_id, now)?.ok()?;
                Some((feed_id, data.clone()))
            })
            .collect::<HashMap<_, _>>()
    }

    fn write_prices(&mut self, feed_ids: Vec<FeedId>, payload: Base64VecU8) {
        let updater = env::predecessor_account_id();

        let is_trusted = <Self as Rbac>::has_role(&updater, &Role::TrustedUpdater);

        let now = Milliseconds::now();

        let payload = self
            .adapter
            .validate_payload(&feed_ids, &payload.0, now)
            .unwrap_or_reject();

        let writes = self.adapter.write_prices(is_trusted, payload, now);

        let updated_feeds = writes
            .into_iter()
            .filter_map(|(feed_id, result)| match result {
                Ok(feed_data) => Some((feed_id, feed_data)),
                Err(e) => {
                    near_sdk::log!("Failed to update feed {feed_id}: {e}");
                    None
                }
            })
            .collect::<HashMap<_, _>>();

        RedStoneEvent::WritePrices {
            updater,
            updated_feeds,
        }
        .emit();
    }
}
