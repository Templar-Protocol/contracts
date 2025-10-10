use near_sdk::{
    env,
    json_types::{Base64VecU8, U128, U64},
    near, AccountId, Allowance, Gas, GasWeight, NearToken, Promise,
};
use std::{num::NonZeroU128, ops::Deref};

#[derive(Debug, Clone)]
#[near(serializers = [json])]
pub struct Transaction {
    pub receiver_id: AccountId,
    pub actions: Box<[Action]>,
}

impl Transaction {
    pub fn to_promise(&self) -> Promise {
        let mut promise = Promise::new(self.receiver_id.clone());

        for action in &self.actions {
            promise = action.add(promise);
        }

        promise
    }
}

#[derive(Debug, Clone)]
#[near(serializers = [json])]
pub struct TransactionList(Box<[Transaction]>);

impl Deref for TransactionList {
    type Target = [Transaction];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl TransactionList {
    pub fn to_promise(&self) -> Promise {
        let mut promise = self
            .0
            .first()
            .unwrap_or_else(|| env::panic_str("empty"))
            .to_promise();

        for tx in &self.0[1..] {
            promise = promise.then(tx.to_promise());
        }

        promise
    }
}

#[derive(Debug, Clone)]
#[near(serializers = [json])]
#[non_exhaustive]
pub enum Action {
    CreateAccount,
    DeployContract {
        code: Base64VecU8,
    },
    FunctionCall {
        function_name: String,
        arguments: Base64VecU8,
        amount: NearToken,
        gas: Gas,
    },
    FunctionCallWeight {
        function_name: String,
        arguments: Base64VecU8,
        amount: NearToken,
        gas: Gas,
        weight: U64,
    },
    Transfer {
        amount: NearToken,
    },
    Stake {
        amount: NearToken,
        public_key: near_sdk::PublicKey,
    },
    AddFullAccessKey {
        public_key: near_sdk::PublicKey,
        nonce: u64,
    },
    AddAccessKey {
        public_key: near_sdk::PublicKey,
        allowance: Option<U128>,
        receiver_id: AccountId,
        function_names: String,
        nonce: u64,
    },
    DeleteKey {
        public_key: near_sdk::PublicKey,
    },
    DeleteAccount {
        beneficiary_id: AccountId,
    },
    // DeployGlobalContract {
    //     code: Vec<u8>,
    // },
    // DeployGlobalContractByAccountId {
    //     code: Vec<u8>,
    // },
    // UseGlobalContract {
    //     code_hash: Vec<u8>,
    // },
    // UseGlobalContractByAccountId {
    //     account_id: AccountId,
    // },
}

impl Action {
    pub fn add(&self, promise: Promise) -> Promise {
        match self {
            Action::CreateAccount => promise.create_account(),
            Action::DeployContract { code } => promise.deploy_contract(code.0.clone()),
            Action::FunctionCall {
                function_name,
                arguments,
                amount,
                gas,
            } => promise.function_call(
                function_name.to_string(),
                arguments.0.clone(),
                *amount,
                *gas,
            ),
            Action::FunctionCallWeight {
                function_name,
                arguments,
                amount,
                gas,
                weight,
            } => promise.function_call_weight(
                function_name.to_string(),
                arguments.0.clone(),
                *amount,
                *gas,
                GasWeight(weight.0),
            ),
            Action::Transfer { amount } => promise.transfer(*amount),
            Action::Stake { amount, public_key } => promise.stake(*amount, public_key.clone()),
            Action::AddFullAccessKey { public_key, nonce } => {
                promise.add_full_access_key_with_nonce(public_key.clone(), *nonce)
            }
            Action::AddAccessKey {
                public_key,
                allowance,
                receiver_id,
                function_names,
                nonce,
            } => {
                let allowance = allowance
                    .and_then(|a| NonZeroU128::new(a.0))
                    .map_or(Allowance::Unlimited, Allowance::Limited);
                promise.add_access_key_allowance_with_nonce(
                    public_key.clone(),
                    allowance,
                    receiver_id.clone(),
                    function_names.clone(),
                    *nonce,
                )
            }
            Action::DeleteKey { public_key } => promise.delete_key(public_key.clone()),
            Action::DeleteAccount { beneficiary_id } => {
                promise.delete_account(beneficiary_id.clone())
            }
        }
    }
}
