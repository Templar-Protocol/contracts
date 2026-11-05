#![allow(clippy::needless_pass_by_value)]

use std::collections::HashMap;

use near_sdk::{
    assert_one_yocto, env,
    json_types::{Base64VecU8, U64},
    near, AccountId, Gas, PanicOnDefault, Promise,
};
use near_sdk_contract_tools::{
    owner::{Owner, OwnerInternal},
    rbac::Rbac,
    Owner, Rbac,
};
use templar_common::{
    contract::list,
    oracle::redstone::{
        Config, ConfigPatch, FeedData, FeedId, GetPrices, RedStoneAdapter,
        RedStoneContractInterface, RedStoneEvent, Role,
    },
    upgrade::{UpgradeSource, MIGRATE_METHOD},
    UnwrapReject,
};
use templar_primitives::{strnum::SU256, time::Nanoseconds};

#[derive(Owner, Rbac, PanicOnDefault)]
#[rbac(roles = "Role")]
#[near(contract_state)]
pub struct Contract {
    pub adapter: RedStoneAdapter,
}

#[near]
impl Contract {
    /// Gas reserved for migration in the atomic self-deploy receipt.
    pub const GAS_FOR_MIGRATE: Gas = Gas::from_tgas(250);

    #[init]
    pub fn new(config: Config, admin_id: AccountId) -> Self {
        config.validate().unwrap_or_reject();
        let mut self_ = Self {
            adapter: RedStoneAdapter::new(b"a", config),
        };

        Owner::init(&mut self_, &admin_id);

        <Self as Rbac>::add_role(&mut self_, &admin_id, &Role::ModifyRoles);
        <Self as Rbac>::add_role(&mut self_, &admin_id, &Role::TrustedUpdater);

        self_
    }

    pub fn get_config(&self) -> &Config {
        &self.adapter.config
    }

    /// Apply a partial verification policy to subsequent payloads, preserving cached feeds.
    #[payable]
    pub fn admin_update_config(&mut self, patch: ConfigPatch) {
        assert_one_yocto();
        Self::require_owner();
        self.adapter.config.apply_patch(patch).unwrap_or_reject();
        RedStoneEvent::ConfigUpdated {
            updater: env::predecessor_account_id(),
            config: &self.adapter.config,
        }
        .emit();
    }

    /// Schedule an atomic self-deploy and migration; completion is receipt-level.
    #[payable]
    pub fn admin_upgrade(&mut self, code: UpgradeSource, migrate_args: Base64VecU8) -> Promise {
        assert_one_yocto();
        Self::require_owner();
        if code.is_empty_code() {
            env::panic_str("admin_upgrade: empty code blob");
        }
        code.deploy_and_migrate(MIGRATE_METHOD, migrate_args, Self::GAS_FOR_MIGRATE)
    }

    /// Bootstrap ownership once without rewriting legacy configuration, roles, or feeds.
    #[private]
    pub fn migrate(&mut self, owner_id: Option<AccountId>) {
        match (Self::slot_is_initialized().exists(), owner_id) {
            (false, Some(owner_id)) => Owner::init(self, &owner_id),
            (false, None) => {
                env::panic_str("migrate: owner_id is required for uninitialized ownership");
            }
            (true, None) => {}
            (true, Some(_)) => env::panic_str("migrate: owner is already initialized"),
        }
    }

    pub fn has_role(&self, account_id: AccountId, role: Role) -> bool {
        <Self as Rbac>::has_role(&account_id, &role)
    }

    #[payable]
    pub fn set_role(
        &mut self,
        account_id: AccountId,
        role: Role,
        set: Option<bool>,
        allow_removing_last_member: Option<bool>,
    ) {
        assert_one_yocto();
        let set = set.unwrap_or(true);
        <Self as Rbac>::require_role(&Role::ModifyRoles);
        if set {
            <Self as Rbac>::add_role(self, &account_id, &role);
        } else {
            let allow_removing_last_member = allow_removing_last_member.unwrap_or(false);
            if !allow_removing_last_member {
                let len = <Self as Rbac>::with_members_of(&role, |r| r.len());
                near_sdk::require!(len > 1, "Cannot remove the last member of a role");
            }
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
            .get_prices(&feed_ids, &payload.0, Nanoseconds::near_timestamp())
            .unwrap_or_reject()
    }

    fn read_prices(&self, feed_ids: Vec<FeedId>) -> HashMap<FeedId, SU256> {
        let now = Nanoseconds::near_timestamp();
        feed_ids
            .into_iter()
            .filter_map(|feed_id| {
                let data = self.adapter.feed_data(&feed_id, now)?.ok()?;
                Some((feed_id, data.price))
            })
            .collect::<HashMap<_, _>>()
    }

    fn read_timestamp(&self, feed_id: FeedId) -> Option<Nanoseconds> {
        let data = self
            .adapter
            .feed_data(&feed_id, Nanoseconds::near_timestamp())?
            .unwrap_or_reject();
        Some(data.package_timestamp)
    }

    fn read_price_data_for_feed(&self, feed_id: FeedId) -> Option<FeedData> {
        let data = self
            .adapter
            .feed_data(&feed_id, Nanoseconds::near_timestamp())?
            .unwrap_or_reject();
        Some(data.clone())
    }

    fn read_price_data(&self, feed_ids: Vec<FeedId>) -> HashMap<FeedId, FeedData> {
        let now = Nanoseconds::near_timestamp();
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

        let now = Nanoseconds::near_timestamp();

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
            .collect::<Vec<_>>();

        RedStoneEvent::WritePrices {
            updater,
            updated_feeds,
        }
        .emit();
    }
}
