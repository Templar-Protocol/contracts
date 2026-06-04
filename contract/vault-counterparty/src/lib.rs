#![allow(clippy::needless_pass_by_value)]

use near_sdk::{
    env,
    json_types::U128,
    near, require,
    serde_json::{self, json},
    AccountId, Gas, NearToken, PanicOnDefault, Promise, PromiseOrValue,
};
use std::collections::BTreeMap;
use std::str::FromStr;
use stellar_xdr::curr::{Limited, Limits, ScAddress, ScVal, WriteXdr};

const ONE_YOCTO: NearToken = NearToken::from_yoctonear(1);
const NO_DEPOSIT: NearToken = NearToken::from_yoctonear(0);
const GAS_MT_TRANSFER_CALL: Gas = Gas::from_tgas(150);
const GAS_MARKET_WITHDRAWAL_REQUEST: Gas = Gas::from_tgas(50);
const GAS_MARKET_WITHDRAWAL_EXECUTE: Gas = Gas::from_tgas(100);
const GAS_INTENTS_MT_TRANSFER: Gas = Gas::from_tgas(50);
const GAS_WITHDRAW_TARGET: Gas = Gas::from_tgas(250);
const GAS_WITHDRAW_BUFFER: Gas = Gas::from_tgas(20);
const INTENTS_CONTRACT: &str = "intents.near";
const BRIDGE_REFUEL_ACCOUNT: &str = "bridge-refuel.hot.tg";
const MARKET_SUPPLY_MSG: &str = "\"Supply\"";
const MAX_STELLAR_RECEIVER_LEN: usize = 256;
const MAX_TOKEN_ID_LEN: usize = 256;
const HOT_DEPOSIT_RECEIVER_HEX_LEN: usize = 64;
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
    pub hot_deposit_receiver_hex: String,
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

#[derive(Debug, Clone)]
#[near(serializers = [borsh])]
struct HotDepositReceiver {
    hex: String,
}

impl HotDepositReceiver {
    fn new(receiver_hex: String) -> Self {
        require!(
            receiver_hex.len() == HOT_DEPOSIT_RECEIVER_HEX_LEN
                && receiver_hex.bytes().all(|b| b.is_ascii_hexdigit()),
            "hot deposit receiver must be 64 hex characters"
        );

        Self { hex: receiver_hex }
    }

    fn as_str(&self) -> &str {
        &self.hex
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
    hot_deposit_receiver: HotDepositReceiver,
    supply_positions: BTreeMap<AccountId, u128>,
    withdrawal_requests: BTreeMap<AccountId, u128>,
}

#[near]
impl Contract {
    #[init]
    pub fn new(
        stellar_receiver: String,
        near_market: AccountId,
        omni_token_id: String,
        omni_contract: AccountId,
        hot_deposit_receiver_hex: String,
        curator: AccountId,
        owner: AccountId,
    ) -> Self {
        let stellar_receiver = StellarReceiver::new(stellar_receiver);
        let omni_token_id = OmniTokenId::new(omni_token_id, &omni_contract);
        let hot_deposit_receiver = HotDepositReceiver::new(hot_deposit_receiver_hex);

        Self {
            stellar_receiver,
            near_market,
            omni_token_id,
            curator,
            owner,
            pending_owner: None,
            omni_contract,
            hot_deposit_receiver,
            supply_positions: BTreeMap::new(),
            withdrawal_requests: BTreeMap::new(),
        }
    }

    pub fn forward_to_market(&self, amount: U128) -> Promise {
        self.assert_curator();
        Self::assert_amount(amount);

        self.call_omni(
            "mt_transfer_call",
            json!({
                "receiver_id": self.near_market,
                "token_id": self.omni_token_id_for_contract(),
                "amount": amount,
                "msg": MARKET_SUPPLY_MSG,
            }),
            GAS_MT_TRANSFER_CALL,
        )
    }

    pub fn request_market_withdrawal(&self, amount: U128) -> Promise {
        self.assert_curator();
        Self::assert_amount(amount);

        self.call_market(
            "create_supply_withdrawal_request",
            json!({
                "amount": amount,
            }),
            GAS_MARKET_WITHDRAWAL_REQUEST,
        )
    }

    pub fn cancel_market_withdrawal(&self) -> Promise {
        self.assert_curator();

        self.call_market(
            "cancel_supply_withdrawal_request",
            json!({}),
            GAS_MARKET_WITHDRAWAL_REQUEST,
        )
    }

    pub fn execute_market_withdrawal(&self, batch_limit: Option<u32>) -> Promise {
        self.assert_curator();

        self.call_market(
            "execute_next_supply_withdrawal_request",
            json!({
                "batch_limit": batch_limit,
            }),
            GAS_MARKET_WITHDRAWAL_EXECUTE,
        )
    }

    pub fn get_supply_position(&self, account_id: AccountId) -> Option<serde_json::Value> {
        self.supply_positions
            .get(&account_id)
            .copied()
            .filter(|amount| *amount > 0)
            .map(Self::supply_position_json)
    }

    pub fn create_supply_withdrawal_request(&mut self, amount: U128) {
        Self::assert_amount(amount);
        let predecessor = env::predecessor_account_id();
        let principal = self
            .supply_positions
            .get(&predecessor)
            .copied()
            .unwrap_or(0);
        require!(
            principal >= amount.0,
            "Attempt to withdraw more than current deposit"
        );
        self.withdrawal_requests.insert(predecessor, amount.0);
    }

    pub fn cancel_supply_withdrawal_request(&mut self) {
        self.withdrawal_requests
            .remove(&env::predecessor_account_id());
    }

    pub fn execute_next_supply_withdrawal_request(
        &mut self,
        _batch_limit: Option<u32>,
    ) -> PromiseOrValue<serde_json::Value> {
        let Some((account_id, requested_amount)) = self
            .withdrawal_requests
            .iter()
            .next()
            .map(|(account_id, amount)| (account_id.clone(), *amount))
        else {
            return PromiseOrValue::Value(Self::withdrawal_execution_result_json(0, 0));
        };

        let principal = self.supply_positions.get(&account_id).copied().unwrap_or(0);
        let amount = requested_amount.min(principal);
        if amount == 0 {
            self.withdrawal_requests.remove(&account_id);
            return PromiseOrValue::Value(Self::withdrawal_execution_result_json(0, 0));
        }

        if requested_amount == amount {
            self.withdrawal_requests.remove(&account_id);
        } else {
            self.withdrawal_requests
                .insert(account_id.clone(), requested_amount - amount);
        }
        self.decrease_supply_position(&account_id, amount);

        PromiseOrValue::Promise(self.intents_transfer_promise(account_id, U128(amount)))
    }

    pub fn withdraw_to_stellar(&self, amount: U128) -> Promise {
        self.assert_curator();
        Self::assert_amount(amount);

        self.hot_withdraw_promise(amount)
    }

    pub fn mt_on_transfer(
        &mut self,
        sender_id: AccountId,
        previous_owner_ids: Vec<AccountId>,
        token_ids: Vec<String>,
        amounts: Vec<U128>,
        msg: String,
    ) -> PromiseOrValue<Vec<U128>> {
        let _authorized_sender = sender_id;

        require!(
            env::predecessor_account_id() == account_id(INTENTS_CONTRACT),
            "Only Intents can transfer-call this adapter"
        );
        require!(
            previous_owner_ids.len() == 1 && token_ids.len() == 1 && amounts.len() == 1,
            "Invalid input length"
        );
        require!(
            token_ids[0] == self.intents_wrapped_token_id(),
            "Unsupported Intents token"
        );
        require!(Self::is_supply_msg(&msg), "Invalid deposit msg");

        let amount = amounts[0];
        Self::assert_amount(amount);

        let supplier = previous_owner_ids[0].clone();
        self.increase_supply_position(supplier, amount.0);
        self.hot_withdraw_promise(amount).detach();

        PromiseOrValue::Value(vec![U128(0)])
    }

    fn hot_withdraw_promise(&self, amount: U128) -> Promise {
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

    fn intents_wrapped_token_id(&self) -> String {
        format!(
            "nep245:{}:{}",
            self.omni_contract,
            self.omni_token_id_for_contract()
        )
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

    fn call_market(&self, method_name: &str, args: serde_json::Value, gas: Gas) -> Promise {
        Promise::new(self.near_market.clone()).function_call(
            method_name.to_string(),
            serde_json::to_vec(&args)
                .unwrap_or_else(|_| env::panic_str("failed to serialize market call args")),
            NO_DEPOSIT,
            gas,
        )
    }

    fn intents_transfer_promise(&self, receiver_id: AccountId, amount: U128) -> Promise {
        Promise::new(account_id(INTENTS_CONTRACT)).function_call(
            "mt_transfer".to_string(),
            serde_json::to_vec(&json!({
                "receiver_id": receiver_id,
                "token_id": self.intents_wrapped_token_id(),
                "amount": amount,
            }))
            .unwrap_or_else(|_| env::panic_str("failed to serialize intents transfer args")),
            ONE_YOCTO,
            GAS_INTENTS_MT_TRANSFER,
        )
    }

    fn omni_token_id_for_contract(&self) -> String {
        self.omni_token_id.as_str().to_string()
    }

    fn increase_supply_position(&mut self, account_id: AccountId, amount: u128) {
        let current = self.supply_positions.get(&account_id).copied().unwrap_or(0);
        let updated = current
            .checked_add(amount)
            .unwrap_or_else(|| env::panic_str("supply position overflow"));
        self.supply_positions.insert(account_id, updated);
    }

    fn decrease_supply_position(&mut self, account_id: &AccountId, amount: u128) {
        let current = self.supply_positions.get(account_id).copied().unwrap_or(0);
        require!(current >= amount, "Insufficient adapter principal");
        let updated = current - amount;
        if updated == 0 {
            self.supply_positions.remove(account_id);
        } else {
            self.supply_positions.insert(account_id.clone(), updated);
        }
    }

    fn supply_position_json(amount: u128) -> serde_json::Value {
        json!({
            "started_at_block_timestamp_ms": "0",
            "borrow_asset_deposit": {
                "active": amount.to_string(),
                "incoming": [],
                "outgoing": "0",
            },
            "borrow_asset_yield": {
                "total": "0",
                "fraction_as_u128_dividend": "0",
                "next_snapshot_index": 0,
            },
        })
    }

    fn withdrawal_execution_result_json(depth: u128, length: u32) -> serde_json::Value {
        json!({
            "depth": depth.to_string(),
            "length": length,
        })
    }

    fn is_supply_msg(msg: &str) -> bool {
        near_sdk::serde_json::from_str::<String>(msg).is_ok_and(|parsed| parsed == "Supply")
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
            hot_deposit_receiver_hex: self.hot_deposit_receiver.as_str().to_string(),
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
    use proptest::prelude::*;

    use super::*;

    const HOT_DEPOSIT_RECEIVER_HEX: &str =
        "52fd581de41f4bace88c936b89bf267a1161426a466adc518cd9e56f201651dd";

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
            HOT_DEPOSIT_RECEIVER_HEX.to_string(),
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

    fn active_position(contract: &Contract, account_id: &AccountId) -> u128 {
        contract
            .supply_positions
            .get(account_id)
            .copied()
            .unwrap_or(0)
    }

    fn catch_contract_panic(call: impl FnOnce()) -> bool {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(call)).is_err()
    }

    fn expected_normalized_token_id(input: &str, omni_contract: &AccountId) -> Option<String> {
        if input.trim().is_empty() || input.len() > MAX_TOKEN_ID_LEN {
            return None;
        }
        let wrapped_prefix = format!("nep245:{}:", omni_contract);
        let normalized = input.strip_prefix(&wrapped_prefix).unwrap_or(input);
        if normalized.contains(':')
            || !normalized.starts_with(HOT_STELLAR_CHAIN_PREFIX)
            || normalized.len() <= HOT_STELLAR_CHAIN_PREFIX.len()
        {
            return None;
        }
        Some(normalized.to_string())
    }

    fn token_suffix_strategy() -> impl Strategy<Value = String> {
        "[A-Za-z0-9_.-]{1,96}"
    }

    #[derive(Clone, Debug)]
    enum CounterpartyOp {
        Deposit(u128),
        Request(u128),
        Execute,
        Cancel,
    }

    fn counterparty_op_strategy() -> impl Strategy<Value = CounterpartyOp> {
        prop_oneof![
            (1u128..=1_000_000u128).prop_map(CounterpartyOp::Deposit),
            (1u128..=1_500_000u128).prop_map(CounterpartyOp::Request),
            Just(CounterpartyOp::Execute),
            Just(CounterpartyOp::Cancel),
        ]
    }

    proptest! {
        #[test]
        fn prop_token_id_config_accepts_only_normalized_hot_stellar_ids(
            token_id in ".{0,320}",
            use_matching_wrapper in any::<bool>(),
        ) {
            let omni_contract = account("v2_1.omni.hot.tg");
            let configured = if use_matching_wrapper
                && !token_id.starts_with("nep245:")
                && token_id.len() <= MAX_TOKEN_ID_LEN.saturating_sub("nep245:v2_1.omni.hot.tg:".len())
            {
                format!("nep245:{}:{token_id}", omni_contract)
            } else {
                token_id
            };
            let expected = expected_normalized_token_id(&configured, &omni_contract);

            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                Contract::new(
                    "GCMVV45LOZUYYVXOQJ626VXGL3KFXY73DHFBT4EDPDBE2LN4USRQDYVV".to_string(),
                    account("templar-market.near"),
                    configured.clone(),
                    omni_contract.clone(),
                    HOT_DEPOSIT_RECEIVER_HEX.to_string(),
                    account("curator.near"),
                    account("owner.near"),
                )
            }));

            match expected {
                Some(normalized) => {
                    let contract = result.expect("valid HOT Stellar token id should initialize");
                    prop_assert_eq!(contract.get_config().omni_token_id, normalized);
                    prop_assert_eq!(
                        contract.intents_wrapped_token_id(),
                        format!("nep245:{}:{}", omni_contract, contract.get_config().omni_token_id)
                    );
                }
                None => {
                    prop_assert!(result.is_err(), "invalid token id was accepted: {configured:?}");
                }
            }
        }

        #[test]
        fn prop_matching_wrapped_token_ids_roundtrip_to_intents_token(suffix in token_suffix_strategy()) {
            let raw = format!("{HOT_STELLAR_CHAIN_PREFIX}{suffix}");
            let wrapped = format!("nep245:v2_1.omni.hot.tg:{raw}");

            let raw_contract = Contract::new(
                "GCMVV45LOZUYYVXOQJ626VXGL3KFXY73DHFBT4EDPDBE2LN4USRQDYVV".to_string(),
                account("templar-market.near"),
                raw.clone(),
                account("v2_1.omni.hot.tg"),
                HOT_DEPOSIT_RECEIVER_HEX.to_string(),
                account("curator.near"),
                account("owner.near"),
            );
            let wrapped_contract = Contract::new(
                "GCMVV45LOZUYYVXOQJ626VXGL3KFXY73DHFBT4EDPDBE2LN4USRQDYVV".to_string(),
                account("templar-market.near"),
                wrapped,
                account("v2_1.omni.hot.tg"),
                HOT_DEPOSIT_RECEIVER_HEX.to_string(),
                account("curator.near"),
                account("owner.near"),
            );

            prop_assert_eq!(raw_contract.get_config().omni_token_id, raw.clone());
            prop_assert_eq!(wrapped_contract.get_config().omni_token_id, raw.clone());
            prop_assert_eq!(
                wrapped_contract.intents_wrapped_token_id(),
                format!("nep245:v2_1.omni.hot.tg:{raw}")
            );
        }

        #[test]
        fn prop_hot_deposit_receiver_accepts_only_64_hex_chars(
            candidate in "[0-9A-Fa-f]{0,80}",
            append_non_hex in any::<bool>(),
        ) {
            let configured = if append_non_hex {
                format!("{candidate}g")
            } else {
                candidate
            };
            let should_accept = configured.len() == HOT_DEPOSIT_RECEIVER_HEX_LEN
                && configured.bytes().all(|b| b.is_ascii_hexdigit());

            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                Contract::new(
                    "GCMVV45LOZUYYVXOQJ626VXGL3KFXY73DHFBT4EDPDBE2LN4USRQDYVV".to_string(),
                    account("templar-market.near"),
                    "1100_stellar_usdc".to_string(),
                    account("v2_1.omni.hot.tg"),
                    configured.clone(),
                    account("curator.near"),
                    account("owner.near"),
                )
            }));

            if should_accept {
                prop_assert!(result.is_ok());
            } else {
                prop_assert!(result.is_err(), "invalid HOT receiver was accepted: {configured:?}");
            }
        }

        #[test]
        fn prop_mt_on_transfer_validation_is_atomic(
            valid_predecessor in any::<bool>(),
            valid_lengths in any::<bool>(),
            valid_token in any::<bool>(),
            valid_msg in any::<bool>(),
            amount in 0u128..=1_000_000u128,
        ) {
            let mut contract = test_contract();
            let supplier = account("vault.near");
            let predecessor = if valid_predecessor {
                account(INTENTS_CONTRACT)
            } else {
                account("attacker.near")
            };
            context(&predecessor);

            let previous_owner_ids = if valid_lengths {
                vec![supplier.clone()]
            } else {
                vec![supplier.clone(), account("other.near")]
            };
            let token_ids = if valid_lengths {
                vec![if valid_token {
                    "nep245:v2_1.omni.hot.tg:1100_stellar_usdc".to_string()
                } else {
                    "1100_stellar_usdc".to_string()
                }]
            } else {
                vec![]
            };
            let amounts = if valid_lengths {
                vec![U128(amount)]
            } else {
                vec![U128(amount), U128(1)]
            };
            let msg = if valid_msg {
                MARKET_SUPPLY_MSG.to_string()
            } else {
                "\"Withdraw\"".to_string()
            };
            let before = active_position(&contract, &supplier);

            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                contract.mt_on_transfer(
                    account(INTENTS_CONTRACT),
                    previous_owner_ids,
                    token_ids,
                    amounts,
                    msg,
                )
            }));

            let should_succeed =
                valid_predecessor && valid_lengths && valid_token && valid_msg && amount > 0;
            if should_succeed {
                let refunds = result.expect("valid transfer-call should succeed");
                prop_assert!(matches!(refunds, PromiseOrValue::Value(ref values) if values == &vec![U128(0)]));
                prop_assert_eq!(active_position(&contract, &supplier), before + amount);
            } else {
                prop_assert!(result.is_err(), "invalid transfer-call unexpectedly succeeded");
                prop_assert_eq!(active_position(&contract, &supplier), before);
            }
        }

        #[test]
        fn prop_supply_withdrawal_queue_matches_simple_model(
            ops in prop::collection::vec(counterparty_op_strategy(), 1..25),
        ) {
            let mut contract = test_contract();
            let supplier = account("vault.near");
            let mut model_principal = 0u128;
            let mut model_request: Option<u128> = None;

            for op in ops {
                match op {
                    CounterpartyOp::Deposit(amount) => {
                        context(&account(INTENTS_CONTRACT));
                        let result = contract.mt_on_transfer(
                            account(INTENTS_CONTRACT),
                            vec![supplier.clone()],
                            vec!["nep245:v2_1.omni.hot.tg:1100_stellar_usdc".to_string()],
                            vec![U128(amount)],
                            MARKET_SUPPLY_MSG.to_string(),
                        );
                        prop_assert!(matches!(result, PromiseOrValue::Value(ref refunds) if refunds == &vec![U128(0)]));
                        model_principal = model_principal.checked_add(amount).expect("bounded test amounts do not overflow");
                    }
                    CounterpartyOp::Request(amount) => {
                        if model_principal == 0 {
                            continue;
                        }
                        let amount = (amount % model_principal) + 1;
                        context(&supplier);
                        contract.create_supply_withdrawal_request(U128(amount));
                        model_request = Some(amount);
                    }
                    CounterpartyOp::Execute => {
                        context(&account("keeper.near"));
                        let result = contract.execute_next_supply_withdrawal_request(Some(1));
                        match model_request {
                            Some(requested) => {
                                let executed = requested.min(model_principal);
                                if executed == 0 {
                                    prop_assert!(matches!(result, PromiseOrValue::Value(_)));
                                    model_request = None;
                                } else {
                                    prop_assert!(matches!(result, PromiseOrValue::Promise(_)));
                                    model_principal -= executed;
                                    model_request = requested.checked_sub(executed).filter(|remaining| *remaining > 0);
                                }
                            }
                            None => {
                                prop_assert!(matches!(result, PromiseOrValue::Value(_)));
                            }
                        }
                    }
                    CounterpartyOp::Cancel => {
                        context(&supplier);
                        contract.cancel_supply_withdrawal_request();
                        model_request = None;
                    }
                }

                prop_assert_eq!(active_position(&contract, &supplier), model_principal);
                prop_assert_eq!(contract.withdrawal_requests.get(&supplier).copied(), model_request);
            }
        }

        #[test]
        fn prop_invalid_supply_withdrawal_request_does_not_mutate(
            principal in 0u128..=1_000_000u128,
            extra in 0u128..=1_000_000u128,
            use_zero_request in any::<bool>(),
        ) {
            let mut contract = test_contract();
            let supplier = account("vault.near");
            if principal > 0 {
                context(&account(INTENTS_CONTRACT));
                let _ = contract.mt_on_transfer(
                    account(INTENTS_CONTRACT),
                    vec![supplier.clone()],
                    vec!["nep245:v2_1.omni.hot.tg:1100_stellar_usdc".to_string()],
                    vec![U128(principal)],
                    MARKET_SUPPLY_MSG.to_string(),
                );
            }

            context(&supplier);
            let request = if use_zero_request {
                0
            } else {
                principal.saturating_add(extra).saturating_add(1)
            };
            let before_principal = active_position(&contract, &supplier);
            let before_request = contract.withdrawal_requests.get(&supplier).copied();

            let panicked = catch_contract_panic(|| {
                contract.create_supply_withdrawal_request(U128(request));
            });

            prop_assert!(panicked);
            prop_assert_eq!(active_position(&contract, &supplier), before_principal);
            prop_assert_eq!(contract.withdrawal_requests.get(&supplier).copied(), before_request);
        }
    }

    #[test]
    fn forward_to_market_builds_expected_mt_transfer_call_supply() {
        let contract = test_contract();
        context(&account("curator.near"));

        let _ = contract.forward_to_market(U128(42));

        let (receiver, method, args, attached_deposit, prepaid_gas) = first_function_call();
        assert_eq!(receiver, account("v2_1.omni.hot.tg"));
        assert_eq!(method, "mt_transfer_call");
        assert_eq!(attached_deposit, ONE_YOCTO);
        assert_eq!(prepaid_gas, GAS_MT_TRANSFER_CALL);
        assert_eq!(args["receiver_id"], "templar-market.near");
        assert_eq!(args["token_id"], "1100_stellar_usdc");
        assert_eq!(args["amount"], "42");
        assert_eq!(args["msg"], MARKET_SUPPLY_MSG);
    }

    #[test]
    fn request_market_withdrawal_builds_market_queue_call() {
        let contract = test_contract();
        context(&account("curator.near"));

        let _ = contract.request_market_withdrawal(U128(42));

        let (receiver, method, args, attached_deposit, prepaid_gas) = first_function_call();
        assert_eq!(receiver, account("templar-market.near"));
        assert_eq!(method, "create_supply_withdrawal_request");
        assert_eq!(attached_deposit, NO_DEPOSIT);
        assert_eq!(prepaid_gas, GAS_MARKET_WITHDRAWAL_REQUEST);
        assert_eq!(args["amount"], "42");
    }

    #[test]
    fn cancel_market_withdrawal_builds_market_queue_call() {
        let contract = test_contract();
        context(&account("curator.near"));

        let _ = contract.cancel_market_withdrawal();

        let (receiver, method, args, attached_deposit, prepaid_gas) = first_function_call();
        assert_eq!(receiver, account("templar-market.near"));
        assert_eq!(method, "cancel_supply_withdrawal_request");
        assert_eq!(attached_deposit, NO_DEPOSIT);
        assert_eq!(prepaid_gas, GAS_MARKET_WITHDRAWAL_REQUEST);
        assert_eq!(args, serde_json::json!({}));
    }

    #[test]
    fn execute_market_withdrawal_builds_market_queue_call() {
        let contract = test_contract();
        context(&account("curator.near"));

        let _ = contract.execute_market_withdrawal(Some(3));

        let (receiver, method, args, attached_deposit, prepaid_gas) = first_function_call();
        assert_eq!(receiver, account("templar-market.near"));
        assert_eq!(method, "execute_next_supply_withdrawal_request");
        assert_eq!(attached_deposit, NO_DEPOSIT);
        assert_eq!(prepaid_gas, GAS_MARKET_WITHDRAWAL_EXECUTE);
        assert_eq!(args["batch_limit"], 3);
    }

    #[test]
    fn adapter_supply_from_vault_records_position_and_bridges_to_stellar() {
        let mut contract = test_contract();
        context(&account(INTENTS_CONTRACT));

        let result = contract.mt_on_transfer(
            account(INTENTS_CONTRACT),
            vec![account("vault.near")],
            vec!["nep245:v2_1.omni.hot.tg:1100_stellar_usdc".to_string()],
            vec![U128(42)],
            MARKET_SUPPLY_MSG.to_string(),
        );

        assert!(matches!(result, PromiseOrValue::Value(ref refunds) if refunds == &vec![U128(0)]));
        let position = contract
            .get_supply_position(account("vault.near"))
            .expect("vault position should be recorded");
        assert_eq!(position["borrow_asset_deposit"]["active"], "42");
        assert_eq!(
            position["borrow_asset_deposit"]["incoming"],
            serde_json::json!([])
        );
        assert_eq!(position["borrow_asset_deposit"]["outgoing"], "0");

        let (receiver, method, args, attached_deposit, _) = first_function_call();
        assert_eq!(receiver, account(INTENTS_CONTRACT));
        assert_eq!(method, "mt_withdraw");
        assert_eq!(attached_deposit, ONE_YOCTO);
        assert_eq!(args["token"], "v2_1.omni.hot.tg");
        assert_eq!(args["receiver_id"], BRIDGE_REFUEL_ACCOUNT);
        assert_eq!(args["token_ids"][0], "1100_stellar_usdc");
        assert_eq!(args["amounts"][0], "42");
    }

    #[test]
    fn adapter_rejects_recomputed_or_wrong_intents_token() {
        let mut contract = test_contract();
        context(&account(INTENTS_CONTRACT));

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract.mt_on_transfer(
                account(INTENTS_CONTRACT),
                vec![account("vault.near")],
                vec!["1100_stellar_usdc".to_string()],
                vec![U128(42)],
                MARKET_SUPPLY_MSG.to_string(),
            )
        }));

        assert!(result.is_err());
    }

    #[test]
    fn adapter_withdrawal_executes_intents_transfer_back_to_vault() {
        let mut contract = test_contract();
        context(&account(INTENTS_CONTRACT));
        let _ = contract.mt_on_transfer(
            account(INTENTS_CONTRACT),
            vec![account("vault.near")],
            vec!["nep245:v2_1.omni.hot.tg:1100_stellar_usdc".to_string()],
            vec![U128(42)],
            MARKET_SUPPLY_MSG.to_string(),
        );

        context(&account("vault.near"));
        contract.create_supply_withdrawal_request(U128(10));

        context(&account("keeper.near"));
        let result = contract.execute_next_supply_withdrawal_request(Some(1));
        let PromiseOrValue::Promise(promise) = result else {
            panic!("expected withdrawal execution promise");
        };
        promise.detach();

        let position = contract
            .get_supply_position(account("vault.near"))
            .expect("remaining vault position should be recorded");
        assert_eq!(position["borrow_asset_deposit"]["active"], "32");
        assert_eq!(
            position["borrow_asset_deposit"]["incoming"],
            serde_json::json!([])
        );
        assert_eq!(position["borrow_asset_deposit"]["outgoing"], "0");

        let (receiver, method, args, attached_deposit, prepaid_gas) = first_function_call();
        assert_eq!(receiver, account(INTENTS_CONTRACT));
        assert_eq!(method, "mt_transfer");
        assert_eq!(attached_deposit, ONE_YOCTO);
        assert_eq!(prepaid_gas, GAS_INTENTS_MT_TRANSFER);
        assert_eq!(args["receiver_id"], "vault.near");
        assert_eq!(
            args["token_id"],
            "nep245:v2_1.omni.hot.tg:1100_stellar_usdc"
        );
        assert_eq!(args["amount"], "10");
    }

    #[test]
    fn wrapped_token_id_is_normalized_for_omni_calls() {
        let contract = Contract::new(
            "GCMVV45LOZUYYVXOQJ626VXGL3KFXY73DHFBT4EDPDBE2LN4USRQDYVV".to_string(),
            account("templar-market.near"),
            "nep245:v2_1.omni.hot.tg:1100_111bzQBB5v7AhLyPMDwS8uJgQV24KaAPXtwyVWu2KXbbfQU6NXRCz"
                .to_string(),
            account("v2_1.omni.hot.tg"),
            HOT_DEPOSIT_RECEIVER_HEX.to_string(),
            account("curator.near"),
            account("owner.near"),
        );
        context(&account("curator.near"));

        let _ = contract.withdraw_to_stellar(U128(1));

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

        let _ = contract.withdraw_to_stellar(U128(999));

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

        let _ = contract.withdraw_to_stellar(U128(1));
    }

    #[test]
    #[should_panic(expected = "Only curator can call this method")]
    fn non_curator_cannot_request_market_withdrawal() {
        let contract = test_contract();
        context(&account("not-curator.near"));

        let _ = contract.request_market_withdrawal(U128(1));
    }

    #[test]
    fn owner_can_rotate_curator() {
        let mut contract = test_contract();

        context(&account("owner.near"));
        contract.set_curator(account("new-curator.near"));

        context(&account("new-curator.near"));
        let _ = contract.withdraw_to_stellar(U128(5));

        let (_, method, _, _, _) = first_function_call();
        assert_eq!(method, "mt_withdraw");
    }

    #[test]
    fn token_id_is_fixed_from_configuration() {
        let contract = test_contract();
        context(&account("curator.near"));
        let _ = contract.forward_to_market(U128(7));
        let (_, _, args, _, _) = first_function_call();
        assert_eq!(args["token_id"], "1100_stellar_usdc");
    }

    #[test]
    #[should_panic(expected = "amount must be > 0")]
    fn rejects_zero_amount() {
        let contract = test_contract();
        context(&account("curator.near"));
        let _ = contract.withdraw_to_stellar(U128(0));
    }

    #[test]
    #[should_panic(expected = "amount must be > 0")]
    fn rejects_zero_market_withdrawal_request() {
        let contract = test_contract();
        context(&account("curator.near"));
        let _ = contract.request_market_withdrawal(U128(0));
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
        assert_eq!(config.hot_deposit_receiver_hex, HOT_DEPOSIT_RECEIVER_HEX);
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
            HOT_DEPOSIT_RECEIVER_HEX.to_string(),
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
            HOT_DEPOSIT_RECEIVER_HEX.to_string(),
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
            HOT_DEPOSIT_RECEIVER_HEX.to_string(),
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
            HOT_DEPOSIT_RECEIVER_HEX.to_string(),
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
            HOT_DEPOSIT_RECEIVER_HEX.to_string(),
            account("curator.near"),
            account("owner.near"),
        );

        assert_eq!(contract.get_config().omni_token_id, "1100_stellar_usdc");
    }
}
