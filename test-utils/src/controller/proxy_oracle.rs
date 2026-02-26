use near_sdk::{serde_json::json, AccountId, AccountIdRef};
use near_workspaces::{Account, Contract};
use templar_common::oracle::{
    proxy::{Oracle, Proxy, Role},
    pyth::{OracleResponse, PriceIdentifier},
};
use tokio::sync::OnceCell;

use crate::{define, get_contract};

use super::ContractController;

pub struct ProxyOracleController {
    pub contract: Contract,
}

impl ContractController for ProxyOracleController {
    fn contract(&self) -> &Contract {
        &self.contract
    }
}

impl ProxyOracleController {
    pub async fn deploy(
        account: Account,
        pyth_id: &AccountIdRef,
        redstone_id: &AccountIdRef,
    ) -> Self {
        static WASM: OnceCell<Vec<u8>> = OnceCell::const_new();

        let wasm = WASM
            .get_or_init(|| get_contract("templar_proxy_oracle_contract", "contract/proxy-oracle"))
            .await;

        let contract = account.deploy(wasm).await.unwrap().unwrap();
        contract
            .call("new")
            .args_json(json!({
                "pyth_id": pyth_id,
                "redstone_id": redstone_id,
            }))
            .transact()
            .await
            .unwrap()
            .unwrap();

        Self { contract }
    }

    define! {
        #[view] pub fn oracle_id(oracle: Oracle) -> AccountId;
        #[view] pub fn list_proxies(offset: Option<u32>, count: Option<u32>) -> Vec<PriceIdentifier>;
        #[view] pub fn get_proxy(id: PriceIdentifier) -> Option<Proxy>;

        #[call(exec, yocto(1))]
        pub fn set_role(account_ids: Vec<AccountId>, roles: Vec<Role>, set: Option<bool>, allow_removing_final_member: Option<bool>);
        #[call(exec, yocto(1))]
        pub fn set_oracle_id(oracle: Oracle, account_id: AccountId);
        #[call(yocto(1))]
        pub fn add_proxy(proxy: Proxy) -> PriceIdentifier;

        #[call]
        pub fn price_feed_exists(price_identifier: PriceIdentifier) -> bool;
        #[call(exec)]
        pub fn price_feed_exists_exec["price_feed_exists"](price_identifier: PriceIdentifier) -> bool;
        #[call(tgas(15))]
        pub fn list_ema_prices_no_older_than(price_ids: Vec<PriceIdentifier>, age: u32) -> OracleResponse;
        #[call(exec, tgas(15))]
        pub fn list_ema_prices_no_older_than_exec["list_ema_prices_no_older_than"](price_ids: Vec<PriceIdentifier>, age: u32) -> OracleResponse;
    }
}
