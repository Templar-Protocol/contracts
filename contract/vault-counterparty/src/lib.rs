#![allow(clippy::needless_pass_by_value)]

use near_sdk::{
    env,
    json_types::U128,
    near, require,
    serde_json::{self, json},
    AccountId, Gas, NearToken, PanicOnDefault, Promise,
};

const ONE_YOCTO: NearToken = NearToken::from_yoctonear(1);
const GAS_MT_TRANSFER: Gas = Gas::from_tgas(50);
const GAS_WITHDRAW: Gas = Gas::from_tgas(80);
const MAX_STELLAR_RECEIVER_LEN: usize = 256;
const MAX_TOKEN_ID_LEN: usize = 256;

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

#[derive(PanicOnDefault)]
#[near(contract_state)]
pub struct Contract {
    stellar_receiver: String,
    near_market: AccountId,
    omni_token_id: String,
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
        Self::assert_receiver(&stellar_receiver);
        Self::assert_token_id(&omni_token_id);

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
                "token_id": self.omni_token_id,
                "amount": amount,
            }),
            GAS_MT_TRANSFER,
        )
    }

    pub fn withdraw_to_stellar(&self, amount: U128) -> Promise {
        self.assert_curator();
        Self::assert_amount(amount);

        self.call_omni(
            "withdraw",
            json!({
                "token_id": self.omni_token_id,
                "receiver_id": self.stellar_receiver,
                "amount": amount,
            }),
            GAS_WITHDRAW,
        )
    }

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
            stellar_receiver: self.stellar_receiver.clone(),
            near_market: self.near_market.clone(),
            omni_token_id: self.omni_token_id.clone(),
            curator: self.curator.clone(),
            owner: self.owner.clone(),
            pending_owner: self.pending_owner.clone(),
            omni_contract: self.omni_contract.clone(),
        }
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

    fn assert_token_id(token_id: &str) {
        require!(!token_id.trim().is_empty(), "token_id cannot be empty");
        require!(
            token_id.len() <= MAX_TOKEN_ID_LEN,
            format!("token_id too long, max {}", MAX_TOKEN_ID_LEN)
        );
    }

    fn assert_receiver(receiver: &str) {
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
        testing_env!(builder.build());
    }

    fn test_contract() -> Contract {
        Contract::new(
            "GCYV3WBXWJY4UVQZ6X3I6LBKAKP4YB6ESQOKJP4MZ2S2BFOFGB2P4D7F".to_string(),
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
    fn withdraw_to_stellar_uses_hardcoded_receiver_and_token() {
        let contract = test_contract();
        context(&account("curator.near"));

        contract.withdraw_to_stellar(U128(999));

        let (receiver, method, args, attached_deposit, prepaid_gas) = first_function_call();
        assert_eq!(receiver, account("v2_1.omni.hot.tg"));
        assert_eq!(method, "withdraw");
        assert_eq!(attached_deposit, ONE_YOCTO);
        assert_eq!(prepaid_gas, GAS_WITHDRAW);
        assert_eq!(
            args["receiver_id"],
            "GCYV3WBXWJY4UVQZ6X3I6LBKAKP4YB6ESQOKJP4MZ2S2BFOFGB2P4D7F"
        );
        assert_eq!(args["token_id"], "1100_stellar_usdc");
        assert_eq!(args["amount"], "999");
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
        assert_eq!(method, "withdraw");
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
            "GCYV3WBXWJY4UVQZ6X3I6LBKAKP4YB6ESQOKJP4MZ2S2BFOFGB2P4D7F".to_string(),
            account("templar-market.near"),
            "".to_string(),
            account("v2_1.omni.hot.tg"),
            account("curator.near"),
            account("owner.near"),
        );
    }
}
