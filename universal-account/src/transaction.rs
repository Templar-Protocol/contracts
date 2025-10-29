use near_sdk::{
    json_types::{Base58CryptoHash, Base64VecU8, U128, U64},
    near, AccountId, Allowance, Gas, GasWeight, NearToken, Promise,
};
use std::num::NonZeroU128;

#[derive(Debug, Clone)]
#[near(serializers = [json])]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct FunctionCallAction {
    pub function_name: String,
    pub arguments: Base64VecU8,
    pub amount: NearToken,
    pub gas: Gas,
}

#[cfg(not(target_arch = "wasm32"))]
impl From<FunctionCallAction> for near_primitives::action::FunctionCallAction {
    fn from(value: FunctionCallAction) -> Self {
        Self {
            method_name: value.function_name,
            args: value.arguments.0,
            gas: value.gas.as_gas(),
            deposit: value.amount.as_yoctonear(),
        }
    }
}

impl From<FunctionCallAction> for Action {
    fn from(value: FunctionCallAction) -> Self {
        Action::FunctionCall(Box::new(value))
    }
}

#[derive(Debug, Clone)]
#[near(serializers = [json])]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub enum Action {
    CreateAccount,
    DeployContract {
        code: Base64VecU8,
    },
    FunctionCall(Box<FunctionCallAction>),
    FunctionCallWeight {
        #[serde(flatten)]
        call: Box<FunctionCallAction>,
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
    DeployGlobalContract {
        code: Base64VecU8,
    },
    DeployGlobalContractByAccountId {
        code: Base64VecU8,
    },
    UseGlobalContract {
        code_hash: Base58CryptoHash,
    },
    UseGlobalContractByAccountId {
        account_id: AccountId,
    },
}

impl Action {
    pub fn add(&self, promise: Promise) -> Promise {
        match self {
            Action::CreateAccount => promise.create_account(),
            Action::DeployContract { code } => promise.deploy_contract(code.0.clone()),
            Action::FunctionCall(ref call) => promise.function_call(
                call.function_name.to_string(),
                call.arguments.0.clone(),
                call.amount,
                call.gas,
            ),
            Action::FunctionCallWeight { call, weight } => promise.function_call_weight(
                call.function_name.to_string(),
                call.arguments.0.clone(),
                call.amount,
                call.gas,
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
            Action::DeployGlobalContract { code } => promise.deploy_global_contract(code.0.clone()),
            Action::DeployGlobalContractByAccountId { code } => {
                promise.deploy_global_contract_by_account_id(code.0.clone())
            }
            Action::UseGlobalContract { code_hash } => {
                promise.use_global_contract(near_sdk::CryptoHash::from(*code_hash).to_vec())
            }
            Action::UseGlobalContractByAccountId { account_id } => {
                promise.use_global_contract_by_account_id(account_id.clone())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use near_primitives::{
        action::{GlobalContractDeployMode, GlobalContractIdentifier},
        hash::CryptoHash,
        types::GasWeight,
    };
    use near_sdk::{
        mock::MockAction,
        test_utils::{self, VMContextBuilder},
    };

    fn public_key(b: u8) -> near_sdk::PublicKey {
        near_sdk::PublicKey::from_parts(near_sdk::CurveType::ED25519, vec![b; 32]).unwrap()
    }

    #[allow(clippy::too_many_lines)]
    #[test]
    fn transaction() {
        let context = VMContextBuilder::new().build();
        near_sdk::testing_env!(context.clone());

        let t = Transaction {
            receiver_id: "receiver.near".parse().unwrap(),
            actions: vec![
                Action::CreateAccount,
                Action::DeployContract {
                    code: [1u8; 32].to_vec().into(),
                },
                FunctionCallAction {
                    function_name: "function_call".to_string(),
                    arguments: b"args2".to_vec().into(),
                    amount: NearToken::from_near(2),
                    gas: near_sdk::Gas::from_tgas(20),
                }
                .into(),
                Action::FunctionCallWeight {
                    call: Box::new(FunctionCallAction {
                        function_name: "function_call_weight".to_string(),
                        arguments: b"args3".to_vec().into(),
                        amount: NearToken::from_near(3),
                        gas: near_sdk::Gas::from_tgas(30),
                    }),
                    weight: U64(3),
                },
                Action::Transfer {
                    amount: NearToken::from_near(4),
                },
                Action::Stake {
                    amount: NearToken::from_near(5),
                    public_key: public_key(5),
                },
                Action::AddFullAccessKey {
                    public_key: public_key(6),
                    nonce: 6,
                },
                Action::AddAccessKey {
                    public_key: public_key(7),
                    allowance: Some(U128(NearToken::from_near(7).as_yoctonear())),
                    receiver_id: "access_key7.near".parse().unwrap(),
                    function_names: "access_key7".to_string(),
                    nonce: 7,
                },
                Action::DeleteKey {
                    public_key: public_key(8),
                },
                Action::DeleteAccount {
                    beneficiary_id: "beneficiary9.near".parse().unwrap(),
                },
                Action::DeployGlobalContract {
                    code: vec![10; 32].into(),
                },
                Action::DeployGlobalContractByAccountId {
                    code: vec![11; 32].into(),
                },
                Action::UseGlobalContract {
                    code_hash: [12; 32].into(),
                },
                Action::UseGlobalContractByAccountId {
                    account_id: "useglobalbyaccountid13.near".parse().unwrap(),
                },
            ]
            .into(),
        };

        t.to_promise();

        let receipts = test_utils::get_created_receipts();

        assert_eq!(receipts.len(), 1);
        let receipt = &receipts[0];

        assert_eq!(&receipt.receiver_id, "receiver.near");
        assert_eq!(receipt.receipt_indices.len(), 0);
        assert_eq!(
            receipt.actions,
            vec![
                MockAction::CreateAccount { receipt_index: 0 },
                MockAction::DeployContract {
                    receipt_index: 0,
                    code: vec![1; 32],
                },
                MockAction::FunctionCallWeight {
                    receipt_index: 0,
                    method_name: b"function_call".to_vec(),
                    args: b"args2".to_vec(),
                    attached_deposit: NearToken::from_near(2),
                    prepaid_gas: near_sdk::Gas::from_tgas(20),
                    gas_weight: GasWeight(0),
                },
                MockAction::FunctionCallWeight {
                    receipt_index: 0,
                    method_name: b"function_call_weight".to_vec(),
                    args: b"args3".to_vec(),
                    attached_deposit: NearToken::from_near(3),
                    prepaid_gas: near_sdk::Gas::from_tgas(30),
                    gas_weight: GasWeight(3),
                },
                MockAction::Transfer {
                    receipt_index: 0,
                    deposit: NearToken::from_near(4),
                },
                MockAction::Stake {
                    receipt_index: 0,
                    stake: NearToken::from_near(5),
                    public_key: String::from(&public_key(5)).parse().unwrap(),
                },
                MockAction::AddKeyWithFullAccess {
                    receipt_index: 0,
                    public_key: String::from(&public_key(6)).parse().unwrap(),
                    nonce: 6,
                },
                MockAction::AddKeyWithFunctionCall {
                    receipt_index: 0,
                    public_key: String::from(&public_key(7)).parse().unwrap(),
                    nonce: 7,
                    allowance: Some(NearToken::from_near(7)),
                    receiver_id: "access_key7.near".parse().unwrap(),
                    method_names: vec!["access_key7".to_string()],
                },
                MockAction::DeleteKey {
                    receipt_index: 0,
                    public_key: String::from(&public_key(8)).parse().unwrap(),
                },
                MockAction::DeleteAccount {
                    receipt_index: 0,
                    beneficiary_id: "beneficiary9.near".parse().unwrap(),
                },
                MockAction::DeployGlobalContract {
                    receipt_index: 0,
                    code: vec![10; 32],
                    mode: GlobalContractDeployMode::CodeHash,
                },
                MockAction::DeployGlobalContract {
                    receipt_index: 0,
                    code: vec![11; 32],
                    mode: GlobalContractDeployMode::AccountId,
                },
                MockAction::UseGlobalContract {
                    receipt_index: 0,
                    contract_id: GlobalContractIdentifier::CodeHash(CryptoHash([12; 32])),
                },
                MockAction::UseGlobalContract {
                    receipt_index: 0,
                    contract_id: GlobalContractIdentifier::AccountId(
                        "useglobalbyaccountid13.near".parse().unwrap(),
                    ),
                },
            ],
        );
    }
}
