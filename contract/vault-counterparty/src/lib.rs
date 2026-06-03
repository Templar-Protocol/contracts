#![allow(clippy::needless_pass_by_value)]

use near_sdk::{
    env,
    json_types::U128,
    near, require,
    serde_json::{self, json},
    AccountId, Gas, NearToken, PanicOnDefault, Promise,
};
use std::str::FromStr;
use stellar_xdr::curr::{Limited, Limits, ScAddress, ScVal, WriteXdr};

const ONE_YOCTO: NearToken = NearToken::from_yoctonear(1);
const GAS_MT_TRANSFER: Gas = Gas::from_tgas(50);
const GAS_WITHDRAW_TARGET: Gas = Gas::from_tgas(250);
const GAS_WITHDRAW_BUFFER: Gas = Gas::from_tgas(20);
const INTENTS_CONTRACT: &str = "intents.near";
const BRIDGE_REFUEL_ACCOUNT: &str = "bridge-refuel.hot.tg";
const MAX_STELLAR_RECEIVER_LEN: usize = 256;
const MAX_TOKEN_ID_LEN: usize = 256;
const HOT_STELLAR_CHAIN_PREFIX: &str = "1100_";

#[derive(Debug, Clone)]
#[near(serializers = [json])]
pub struct Config {
    pub stellar_receiver: String,
    pub near_market: AccountId,
    pub omni_token_id: String,
    pub curator: AccountId,
    pub owner: AccountId,
    pub pending_owner: Option<AccountId>,
    pub omni_contract: AccountId,
}

#[derive(Debug, Clone)]
#[near(serializers = [borsh])]
struct StellarReceiver {
    raw: String,
    encoded: String,
}

impl StellarReceiver {
    fn new(receiver: String) -> Self {
        require!(
            !receiver.trim().is_empty(),
            "stellar receiver cannot be empty"
        );
        require!(
            receiver.len() <= MAX_STELLAR_RECEIVER_LEN,
            format!(
                "stellar receiver too long, max {}",
                MAX_STELLAR_RECEIVER_LEN
            )
        );

        let sc_address = ScAddress::from_str(&receiver)
            .unwrap_or_else(|_| env::panic_str("invalid stellar receiver"));
        let encoded = Contract::encode_stellar_sc_address(&sc_address);
        Self {
            raw: receiver,
            encoded,
        }
    }

    fn as_str(&self) -> &str {
        &self.raw
    }

    fn encoded(&self) -> &str {
        &self.encoded
    }
}

#[derive(Debug, Clone)]
#[near(serializers = [borsh])]
struct OmniTokenId(String);

impl OmniTokenId {
    fn new(token_id: String, omni_contract: &AccountId) -> Self {
        require!(!token_id.trim().is_empty(), "token_id cannot be empty");
        require!(
            token_id.len() <= MAX_TOKEN_ID_LEN,
            format!("token_id too long, max {}", MAX_TOKEN_ID_LEN)
        );

        let wrapped_prefix = format!("nep245:{}:", omni_contract);
        let normalized = token_id
            .strip_prefix(&wrapped_prefix)
            .unwrap_or(&token_id)
            .to_string();
        require!(
            !normalized.contains(':'),
            "token_id cannot contain an unexpected wrapper prefix"
        );
        require!(
            normalized.starts_with(HOT_STELLAR_CHAIN_PREFIX)
                && normalized.len() > HOT_STELLAR_CHAIN_PREFIX.len(),
            format!(
                "token_id must start with {} and include an asset id",
                HOT_STELLAR_CHAIN_PREFIX
            )
        );

        Self(normalized)
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(PanicOnDefault)]
#[near(contract_state)]
pub struct Contract {
    stellar_receiver: StellarReceiver,
    near_market: AccountId,
    omni_token_id: OmniTokenId,
    curator: AccountId,
    owner: AccountId,
    pending_owner: Option<AccountId>,
    omni_contract: AccountId,
}

#[near]
impl Contract {
    #[init]
    pub fn new(
        stellar_receiver: String,
        near_market: AccountId,
        omni_token_id: String,
        omni_contract: AccountId,
        curator: AccountId,
        owner: AccountId,
    ) -> Self {
        let stellar_receiver = StellarReceiver::new(stellar_receiver);
        let omni_token_id = OmniTokenId::new(omni_token_id, &omni_contract);

        Self {
            stellar_receiver,
            near_market,
            omni_token_id,
            curator,
            owner,
            pending_owner: None,
            omni_contract,
        }
    }

    pub fn forward_to_market(&self, amount: U128) -> Promise {
        self.assert_curator();
        Self::assert_amount(amount);

        self.call_omni(
            "mt_transfer",
            json!({
                "receiver_id": self.near_market,
                "token_id": self.omni_token_id_for_contract(),
                "amount": amount,
            }),
            GAS_MT_TRANSFER,
        )
    }

    pub fn withdraw_to_stellar(&self, amount: U128) -> Promise {
        self.assert_curator();
        Self::assert_amount(amount);

        let remaining_gas = Gas::from_gas(
            env::prepaid_gas()
                .as_gas()
                .saturating_sub(env::used_gas().as_gas())
                .saturating_sub(GAS_WITHDRAW_BUFFER.as_gas()),
        );
        let forwarded_gas = Gas::from_gas(remaining_gas.as_gas().min(GAS_WITHDRAW_TARGET.as_gas()));

        Promise::new(account_id(INTENTS_CONTRACT)).function_call(
            "mt_withdraw".to_string(),
            serde_json::to_vec(&json!({
                "token": self.intents_token_contract(),
                "receiver_id": BRIDGE_REFUEL_ACCOUNT,
                "token_ids": [self.intents_multi_token_id()],
                "amounts": [amount.0.to_string()],
                "memo": serde_json::Value::Null,
                "msg": self.withdraw_msg_json(),
            }))
            .unwrap_or_else(|_| env::panic_str("failed to serialize withdrawal args")),
            ONE_YOCTO,
            forwarded_gas,
        )
    }

    fn withdraw_msg_json(&self) -> String {
        json!({
            "receiver_id": self.stellar_receiver.encoded(),
            "amount_native": "0",
            "block_number": 0,
        })
        .to_string()
    }

    fn intents_token_contract(&self) -> String {
        self.omni_contract.to_string()
    }

    fn intents_multi_token_id(&self) -> String {
        self.omni_token_id_for_contract()
    }

    fn call_omni(&self, method_name: &str, args: serde_json::Value, gas: Gas) -> Promise {
        Promise::new(self.omni_contract.clone()).function_call(
            method_name.to_string(),
            serde_json::to_vec(&args)
                .unwrap_or_else(|_| env::panic_str("failed to serialize omni call args")),
            ONE_YOCTO,
            gas,
        )
    }

    fn omni_token_id_for_contract(&self) -> String {
        self.omni_token_id.as_str().to_string()
    }

    fn encode_stellar_sc_address(sc_address: &ScAddress) -> String {
        let sc_val = ScVal::Address(sc_address.clone());
        let mut xdr_bytes = Vec::new();
        let mut limited_writer = Limited::new(&mut xdr_bytes, Limits::none());
        sc_val
            .write_xdr(&mut limited_writer)
            .unwrap_or_else(|_| env::panic_str("failed to encode stellar receiver"));
        bs58::encode(xdr_bytes).into_string()
    }
}

fn account_id(value: &str) -> AccountId {
    value
        .parse()
        .unwrap_or_else(|_| env::panic_str("invalid account id constant"))
}

#[near]
impl Contract {
    pub fn set_curator(&mut self, curator: AccountId) {
        self.assert_owner();
        self.curator = curator;
        env::log_str("curator_updated");
    }

    pub fn propose_owner(&mut self, pending_owner: AccountId) {
        self.assert_owner();
        require!(pending_owner != self.owner, "new owner must differ");
        self.pending_owner = Some(pending_owner);
        env::log_str("owner_proposed");
    }

    pub fn accept_owner(&mut self) {
        let predecessor = env::predecessor_account_id();
        require!(
            self.pending_owner
                .as_ref()
                .is_some_and(|pending| pending == &predecessor),
            "Only pending owner can accept ownership"
        );
        self.owner = predecessor;
        self.pending_owner = None;
        env::log_str("owner_accepted");
    }

    pub fn get_config(&self) -> Config {
        Config {
            stellar_receiver: self.stellar_receiver.as_str().to_string(),
            near_market: self.near_market.clone(),
            omni_token_id: self.omni_token_id.as_str().to_string(),
            curator: self.curator.clone(),
            owner: self.owner.clone(),
            pending_owner: self.pending_owner.clone(),
            omni_contract: self.omni_contract.clone(),
        }
    }

    fn assert_curator(&self) {
        require!(
            env::predecessor_account_id() == self.curator,
            "Only curator can call this method"
        );
    }

    fn assert_owner(&self) {
        require!(
            env::predecessor_account_id() == self.owner,
            "Only owner can call this method"
        );
    }

    fn assert_amount(amount: U128) {
        require!(amount.0 > 0, "amount must be > 0");
    }
}

#[cfg(test)]
mod tests {
    use near_sdk::{
        mock::MockAction,
        serde_json::Value,
        test_utils::{get_created_receipts, VMContextBuilder},
        testing_env, AccountId,
    };

    use super::*;

    fn account(account_id: &str) -> AccountId {
        account_id
            .parse()
            .unwrap_or_else(|_| panic!("invalid account id: {account_id}"))
    }

    fn context(predecessor: &AccountId) {
        let mut builder = VMContextBuilder::new();
        builder.current_account_id(account("counterparty.near"));
        builder.predecessor_account_id(predecessor.clone());
        builder.signer_account_id(predecessor.clone());
        builder.prepaid_gas(Gas::from_tgas(400));
        testing_env!(builder.build());
    }

    fn test_contract() -> Contract {
        Contract::new(
            "GCMVV45LOZUYYVXOQJ626VXGL3KFXY73DHFBT4EDPDBE2LN4USRQDYVV".to_string(),
            account("templar-market.near"),
            "1100_stellar_usdc".to_string(),
            account("v2_1.omni.hot.tg"),
            account("curator.near"),
            account("owner.near"),
        )
    }

    fn first_function_call() -> (AccountId, String, Value, NearToken, Gas) {
        let receipts = get_created_receipts();
        assert_eq!(receipts.len(), 1, "expected exactly one outgoing receipt");

        let receipt = &receipts[0];
        assert_eq!(receipt.actions.len(), 1, "expected exactly one action");

        let MockAction::FunctionCallWeight {
            method_name,
            args,
            attached_deposit,
            prepaid_gas,
            ..
        } = &receipt.actions[0]
        else {
            panic!("expected FunctionCallWeight action")
        };

        let method_name = String::from_utf8(method_name.clone()).unwrap_or_else(|e| panic!("{e}"));
        let args: Value = serde_json::from_slice(args).unwrap_or_else(|e| panic!("{e}"));

        (
            receipt.receiver_id.clone(),
            method_name,
            args,
            *attached_deposit,
            *prepaid_gas,
        )
    }

    #[test]
    fn forward_to_market_builds_expected_mt_transfer_call() {
        let contract = test_contract();
        context(&account("curator.near"));

        contract.forward_to_market(U128(42));

        let (receiver, method, args, attached_deposit, prepaid_gas) = first_function_call();
        assert_eq!(receiver, account("v2_1.omni.hot.tg"));
        assert_eq!(method, "mt_transfer");
        assert_eq!(attached_deposit, ONE_YOCTO);
        assert_eq!(prepaid_gas, GAS_MT_TRANSFER);
        assert_eq!(args["receiver_id"], "templar-market.near");
        assert_eq!(args["token_id"], "1100_stellar_usdc");
        assert_eq!(args["amount"], "42");
    }

    #[test]
    fn wrapped_token_id_is_normalized_for_omni_calls() {
        let contract = Contract::new(
            "GCMVV45LOZUYYVXOQJ626VXGL3KFXY73DHFBT4EDPDBE2LN4USRQDYVV".to_string(),
            account("templar-market.near"),
            "nep245:v2_1.omni.hot.tg:1100_111bzQBB5v7AhLyPMDwS8uJgQV24KaAPXtwyVWu2KXbbfQU6NXRCz"
                .to_string(),
            account("v2_1.omni.hot.tg"),
            account("curator.near"),
            account("owner.near"),
        );
        context(&account("curator.near"));

        contract.withdraw_to_stellar(U128(1));

        let (_, _, args, _, _) = first_function_call();
        assert_eq!(
            args["token_ids"][0],
            "1100_111bzQBB5v7AhLyPMDwS8uJgQV24KaAPXtwyVWu2KXbbfQU6NXRCz"
        );
    }

    #[test]
    fn withdraw_to_stellar_uses_hardcoded_receiver_and_token() {
        let contract = test_contract();
        context(&account("curator.near"));

        contract.withdraw_to_stellar(U128(999));

        let (receiver, method, args, attached_deposit, prepaid_gas) = first_function_call();
        assert_eq!(receiver, account(INTENTS_CONTRACT));
        assert_eq!(method, "mt_withdraw");
        assert_eq!(attached_deposit, ONE_YOCTO);
        assert_eq!(prepaid_gas, GAS_WITHDRAW_TARGET);
        assert_eq!(args["token"], "v2_1.omni.hot.tg");
        assert_eq!(args["receiver_id"], BRIDGE_REFUEL_ACCOUNT);
        assert_eq!(args["token_ids"][0], "1100_stellar_usdc");
        assert_eq!(args["amounts"][0], "999");
        let msg: serde_json::Value = serde_json::from_str(
            args["msg"]
                .as_str()
                .unwrap_or_else(|| panic!("missing withdrawal msg")),
        )
        .unwrap_or_else(|e| panic!("failed to parse withdrawal msg: {e}"));
        assert_eq!(
            msg["receiver_id"],
            Contract::encode_stellar_sc_address(
                &ScAddress::from_str("GCMVV45LOZUYYVXOQJ626VXGL3KFXY73DHFBT4EDPDBE2LN4USRQDYVV")
                    .unwrap_or_else(|_| panic!("invalid test stellar receiver"))
            )
        );
        assert_eq!(msg["amount_native"], "0");
        assert_eq!(msg["block_number"], 0);
    }

    #[test]
    #[should_panic(expected = "Only curator can call this method")]
    fn non_curator_cannot_withdraw_to_stellar() {
        let contract = test_contract();
        context(&account("not-curator.near"));

        contract.withdraw_to_stellar(U128(1));
    }

    #[test]
    fn owner_can_rotate_curator() {
        let mut contract = test_contract();

        context(&account("owner.near"));
        contract.set_curator(account("new-curator.near"));

        context(&account("new-curator.near"));
        contract.withdraw_to_stellar(U128(5));

        let (_, method, _, _, _) = first_function_call();
        assert_eq!(method, "mt_withdraw");
    }

    #[test]
    fn token_id_is_fixed_from_configuration() {
        let contract = test_contract();
        context(&account("curator.near"));
        contract.forward_to_market(U128(7));
        let (_, _, args, _, _) = first_function_call();
        assert_eq!(args["token_id"], "1100_stellar_usdc");
    }

    #[test]
    #[should_panic(expected = "amount must be > 0")]
    fn rejects_zero_amount() {
        let contract = test_contract();
        context(&account("curator.near"));
        contract.withdraw_to_stellar(U128(0));
    }

    #[test]
    #[should_panic(expected = "Only owner can call this method")]
    fn non_owner_cannot_rotate_curator() {
        let mut contract = test_contract();
        context(&account("curator.near"));
        contract.set_curator(account("attacker.near"));
    }

    #[test]
    fn ownership_transfer_is_two_step() {
        let mut contract = test_contract();
        context(&account("owner.near"));
        contract.propose_owner(account("new-owner.near"));

        context(&account("new-owner.near"));
        contract.accept_owner();

        let config = contract.get_config();
        assert_eq!(config.owner, account("new-owner.near"));
        assert_eq!(config.pending_owner, None);
        assert_eq!(config.near_market, account("templar-market.near"));
        assert_eq!(config.omni_token_id, "1100_stellar_usdc");
        assert_eq!(config.omni_contract, account("v2_1.omni.hot.tg"));
    }

    #[test]
    #[should_panic(expected = "Only pending owner can accept ownership")]
    fn non_pending_owner_cannot_accept_ownership() {
        let mut contract = test_contract();
        context(&account("owner.near"));
        contract.propose_owner(account("new-owner.near"));

        context(&account("someone-else.near"));
        contract.accept_owner();
    }

    #[test]
    #[should_panic(expected = "token_id cannot be empty")]
    fn init_rejects_empty_token_id() {
        let _ = Contract::new(
            "GCMVV45LOZUYYVXOQJ626VXGL3KFXY73DHFBT4EDPDBE2LN4USRQDYVV".to_string(),
            account("templar-market.near"),
            "".to_string(),
            account("v2_1.omni.hot.tg"),
            account("curator.near"),
            account("owner.near"),
        );
    }

    #[test]
    #[should_panic(expected = "invalid stellar receiver")]
    fn init_rejects_invalid_stellar_receiver() {
        let _ = Contract::new(
            "not-a-stellar-address".to_string(),
            account("templar-market.near"),
            "1100_stellar_usdc".to_string(),
            account("v2_1.omni.hot.tg"),
            account("curator.near"),
            account("owner.near"),
        );
    }

    #[test]
    #[should_panic(expected = "token_id must start with 1100_ and include an asset id")]
    fn init_rejects_token_id_for_wrong_chain() {
        let _ = Contract::new(
            "GCMVV45LOZUYYVXOQJ626VXGL3KFXY73DHFBT4EDPDBE2LN4USRQDYVV".to_string(),
            account("templar-market.near"),
            "1101_stellar_usdc".to_string(),
            account("v2_1.omni.hot.tg"),
            account("curator.near"),
            account("owner.near"),
        );
    }

    #[test]
    #[should_panic(expected = "token_id cannot contain an unexpected wrapper prefix")]
    fn init_rejects_unexpected_token_wrapper_prefix() {
        let _ = Contract::new(
            "GCMVV45LOZUYYVXOQJ626VXGL3KFXY73DHFBT4EDPDBE2LN4USRQDYVV".to_string(),
            account("templar-market.near"),
            "nep245:other.omni.hot.tg:1100_stellar_usdc".to_string(),
            account("v2_1.omni.hot.tg"),
            account("curator.near"),
            account("owner.near"),
        );
    }

    #[test]
    fn init_normalizes_expected_wrapped_token_id() {
        let contract = Contract::new(
            "GCMVV45LOZUYYVXOQJ626VXGL3KFXY73DHFBT4EDPDBE2LN4USRQDYVV".to_string(),
            account("templar-market.near"),
            "nep245:v2_1.omni.hot.tg:1100_stellar_usdc".to_string(),
            account("v2_1.omni.hot.tg"),
            account("curator.near"),
            account("owner.near"),
        );

        assert_eq!(contract.get_config().omni_token_id, "1100_stellar_usdc");
    }
}
