use near_sdk::{
    json_types::{Base58CryptoHash, Base64VecU8, U128, U64},
    near, AccountId, Allowance, Gas, GasWeight, NearToken, Promise,
};
use std::num::NonZeroU128;

#[derive(Debug, Clone)]
#[near(serializers = [borsh, json])]
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
#[near(serializers = [borsh, json])]
#[serde(deny_unknown_fields)]
pub struct FunctionCallAction {
    pub function_name: String,
    pub arguments: Base64VecU8,
    pub amount: NearToken,
    pub gas: Gas,
}

impl FunctionCallAction {
    pub fn new(
        function_name: impl Into<String>,
        arguments: impl Into<Vec<u8>>,
        amount: NearToken,
        gas: Gas,
    ) -> Self {
        Self {
            function_name: function_name.into(),
            arguments: Base64VecU8(arguments.into()),
            amount,
            gas,
        }
    }
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
#[near(serializers = [borsh, json])]
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
    #[cfg(not(target_arch = "wasm32"))]
    pub fn gas_cost(
        &self,
        receiver_id: &near_sdk::AccountIdRef,
        sir: bool,
        protocol_config: &near_chain_configs::ProtocolConfigView,
    ) -> Gas {
        let fee =
            |f: &near_parameters::Fee| f.execution + if sir { f.send_sir } else { f.send_not_sir };

        let costs = &protocol_config
            .runtime_config
            .transaction_costs
            .action_creation_config;
        Gas::from_gas(match self {
            Self::CreateAccount => fee(&costs.create_account_cost),
            Self::DeployContract { code } => {
                fee(&costs.deploy_contract_cost)
                    + fee(&costs.deploy_contract_cost_per_byte) * code.0.len() as u64
            }
            Self::FunctionCall(call) | Action::FunctionCallWeight { call, .. } => {
                let attached_gas = call.gas.as_gas();
                fee(&costs.function_call_cost)
                    + fee(&costs.function_call_cost_per_byte)
                        * (call.function_name.len() + call.arguments.0.len()) as u64
                    + attached_gas
            }
            Self::Transfer { .. } => {
                if receiver_id.get_account_type().is_implicit() {
                    fee(&costs.create_account_cost)
                        + fee(&costs.transfer_cost)
                        + fee(&costs.add_key_cost.full_access_cost)
                } else {
                    fee(&costs.transfer_cost)
                }
            }
            Self::Stake { .. } => fee(&costs.stake_cost),
            Self::AddFullAccessKey { .. } => fee(&costs.add_key_cost.full_access_cost),
            Self::AddAccessKey { function_names, .. } => {
                fee(&costs.add_key_cost.function_call_cost)
                    + fee(&costs.add_key_cost.function_call_cost_per_byte)
                        * (function_names.len() as u64 + 1)
            }
            Self::DeleteKey { .. } => fee(&costs.delete_key_cost),
            Self::DeleteAccount { beneficiary_id } => {
                fee(&costs.delete_account_cost)
                    + fee(&protocol_config
                        .runtime_config
                        .transaction_costs
                        .action_receipt_creation_config)
                    + Self::Transfer {
                        amount: NearToken::from_near(0),
                    }
                    .gas_cost(beneficiary_id, false, protocol_config)
                    .as_gas()
            }
            Self::DeployGlobalContract { code }
            | Self::DeployGlobalContractByAccountId { code } => {
                (fee(&costs.deploy_contract_cost)
                    + fee(&costs.deploy_contract_cost_per_byte) * code.0.len() as u64)
                    * 10
            }
            Self::UseGlobalContract { code_hash } => {
                fee(&costs.deploy_contract_cost)
                    + fee(&costs.deploy_contract_cost_per_byte)
                        * near_sdk::CryptoHash::from(*code_hash).len() as u64
            }
            Self::UseGlobalContractByAccountId { account_id } => {
                fee(&costs.deploy_contract_cost)
                    + fee(&costs.deploy_contract_cost_per_byte) * account_id.len() as u64
            }
        })
    }

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

    #[rstest::rstest]
    #[case(Action::CreateAccount, "alice.near".parse().unwrap(), false, 7_700_000_000_000)]
    #[case(Action::DeployContract { code: [1u8; 32].to_vec().into() }, "alice.near".parse().unwrap(), false, 373_123_713_088)]
    #[case(Action::FunctionCall(FunctionCallAction {
        function_name: "execute".into(),
        arguments: b"{}".to_vec().into(),
        amount: NearToken::from_near(0),
        gas: near_sdk::Gas::from_tgas(300),
    }.into()), "alice.near".parse().unwrap(), false, 300_980_449_276_841)]
    #[case(Action::Transfer { amount: NearToken::from_near(1) }, "alice.near".parse().unwrap(), false, 230_246_125_000)]
    #[case(Action::Stake { amount: NearToken::from_near(1_000), public_key: "ed25519:6E8sCci9badyRkXb3JoRpBj5p8C6Tw41ELDZoiihKEtp".parse().unwrap() }, "alice.near".parse().unwrap(), false, 243_933_312_500)]
    #[case(Action::AddFullAccessKey { public_key: "ed25519:6E8sCci9badyRkXb3JoRpBj5p8C6Tw41ELDZoiihKEtp".parse().unwrap(), nonce: 1234 }, "alice.near".parse().unwrap(), false, 203_530_250_000)]
    #[case(Action::AddAccessKey {
            public_key: "ed25519:6E8sCci9badyRkXb3JoRpBj5p8C6Tw41ELDZoiihKEtp".parse().unwrap(),
            allowance: Some(U128(1234)),
            receiver_id: "contract.near".parse().unwrap(),
            function_names: "fn_1,fn_2".into(),
            nonce: 12345,
        }, "alice.near".parse().unwrap(), false, 204_931_340_460)]
    #[case(Action::DeleteKey { public_key: "ed25519:6E8sCci9badyRkXb3JoRpBj5p8C6Tw41ELDZoiihKEtp".parse().unwrap() }, "alice.near".parse().unwrap(), false, 189_893_250_000)]
    #[case(Action::DeleteAccount { beneficiary_id: "bob.near".parse().unwrap() }, "alice.near".parse().unwrap(), false, 741_343_125_000)]
    #[case(Action::DeployGlobalContract { code: vec![1u8; 128].into() }, "alice.near".parse().unwrap(), false, 3_839_003_523_520)]
    #[case(Action::DeployGlobalContractByAccountId { code: vec![6u8; 128].into() }, "alice.near".parse().unwrap(), false, 3_839_003_523_520)]
    #[case(Action::UseGlobalContract { code_hash: [1u8; 32].into() }, "alice.near".parse().unwrap(), false, 373_123_713_088)]
    #[case(Action::UseGlobalContractByAccountId { account_id: "global".parse().unwrap() }, "alice.near".parse().unwrap(), false, 370_205_039_954)]
    #[test]
    fn gas_cost(
        #[case] action: Action,
        #[case] receiver_id: AccountId,
        #[case] sir: bool,
        #[case] expected_cost: u64,
    ) {
        let config = r#"{"avg_hidden_validator_seats_per_shard":[0,0,0,0,0,0,0,0,0],"block_producer_kickout_threshold":80,"chain_id":"mainnet","chunk_producer_kickout_threshold":80,"chunk_validator_only_kickout_threshold":70,"dynamic_resharding":false,"epoch_length":43200,"fishermen_threshold":"340282366920938463463374607431768211455","gas_limit":1000000000000000,"gas_price_adjustment_rate":[1,100],"genesis_height":9820210,"genesis_time":"2020-07-21T16:55:51.591948Z","max_gas_price":"10000000000000000000000","max_inflation_rate":[1,40],"max_kickout_stake_perc":30,"min_gas_price":"1000000000","minimum_stake_divisor":10,"minimum_stake_ratio":[1,62500],"minimum_validators_per_shard":1,"num_block_producer_seats":100,"num_block_producer_seats_per_shard":[100,100,100,100,100,100,100,100,100],"num_blocks_per_year":31536000,"online_max_threshold":[99,100],"online_min_threshold":[90,100],"protocol_reward_rate":[0,1],"protocol_treasury_account":"treasury.near","protocol_upgrade_stake_threshold":[4,5],"protocol_version":81,"runtime_config":{"account_creation_config":{"min_allowed_top_level_account_length":65,"registrar_account_id":"registrar"},"congestion_control_config":{"allowed_shard_outgoing_gas":1000000000000000,"max_congestion_incoming_gas":400000000000000000,"max_congestion_memory_consumption":1000000000,"max_congestion_missed_chunks":125,"max_congestion_outgoing_gas":10000000000000000,"max_outgoing_gas":300000000000000000,"max_tx_gas":500000000000000,"min_outgoing_gas":1000000000000000,"min_tx_gas":20000000000000,"outgoing_receipts_big_size_limit":4718592,"outgoing_receipts_usual_size_limit":102400,"reject_tx_congestion_threshold":0.8},"storage_amount_per_byte":"10000000000000000000","transaction_costs":{"action_creation_config":{"add_key_cost":{"full_access_cost":{"execution":101765125000,"send_not_sir":101765125000,"send_sir":101765125000},"function_call_cost":{"execution":102217625000,"send_not_sir":102217625000,"send_sir":102217625000},"function_call_cost_per_byte":{"execution":1925331,"send_not_sir":47683715,"send_sir":1925331}},"create_account_cost":{"execution":3850000000000,"send_not_sir":3850000000000,"send_sir":3850000000000},"delegate_cost":{"execution":200000000000,"send_not_sir":200000000000,"send_sir":200000000000},"delete_account_cost":{"execution":147489000000,"send_not_sir":147489000000,"send_sir":147489000000},"delete_key_cost":{"execution":94946625000,"send_not_sir":94946625000,"send_sir":94946625000},"deploy_contract_cost":{"execution":184765750000,"send_not_sir":184765750000,"send_sir":184765750000},"deploy_contract_cost_per_byte":{"execution":64572944,"send_not_sir":47683715,"send_sir":6812999},"function_call_cost":{"execution":780000000000,"send_not_sir":200000000000,"send_sir":200000000000},"function_call_cost_per_byte":{"execution":2235934,"send_not_sir":47683715,"send_sir":2235934},"stake_cost":{"execution":102217625000,"send_not_sir":141715687500,"send_sir":141715687500},"transfer_cost":{"execution":115123062500,"send_not_sir":115123062500,"send_sir":115123062500}},"action_receipt_creation_config":{"execution":108059500000,"send_not_sir":108059500000,"send_sir":108059500000},"burnt_gas_reward":[3,10],"data_receipt_creation_config":{"base_cost":{"execution":36486732312,"send_not_sir":36486732312,"send_sir":36486732312},"cost_per_byte":{"execution":17212011,"send_not_sir":47683715,"send_sir":17212011}},"pessimistic_gas_price_inflation_ratio":[1,1],"storage_usage_config":{"num_bytes_account":100,"num_extra_bytes_record":40}},"wasm_config":{"discard_custom_sections":true,"eth_implicit_accounts":true,"ext_costs":{"alt_bn128_g1_multiexp_base":713000000000,"alt_bn128_g1_multiexp_element":320000000000,"alt_bn128_g1_sum_base":3000000000,"alt_bn128_g1_sum_element":5000000000,"alt_bn128_pairing_check_base":9686000000000,"alt_bn128_pairing_check_element":5102000000000,"base":264768111,"bls12381_g1_multiexp_base":16500000000,"bls12381_g1_multiexp_element":930000000000,"bls12381_g2_multiexp_base":18600000000,"bls12381_g2_multiexp_element":1995000000000,"bls12381_map_fp2_to_g2_base":1500000000,"bls12381_map_fp2_to_g2_element":900000000000,"bls12381_map_fp_to_g1_base":1500000000,"bls12381_map_fp_to_g1_element":252000000000,"bls12381_p1_decompress_base":15000000000,"bls12381_p1_decompress_element":81000000000,"bls12381_p1_sum_base":16500000000,"bls12381_p1_sum_element":6000000000,"bls12381_p2_decompress_base":15000000000,"bls12381_p2_decompress_element":165000000000,"bls12381_p2_sum_base":18600000000,"bls12381_p2_sum_element":15000000000,"bls12381_pairing_base":2130000000000,"bls12381_pairing_element":2130000000000,"contract_compile_base":0,"contract_compile_bytes":0,"contract_loading_base":35445963,"contract_loading_bytes":1089295,"ecrecover_base":278821988457,"ed25519_verify_base":210000000000,"ed25519_verify_byte":9000000,"keccak256_base":5879491275,"keccak256_byte":21471105,"keccak512_base":5811388236,"keccak512_byte":36649701,"log_base":3543313050,"log_byte":13198791,"promise_and_base":1465013400,"promise_and_per_promise":5452176,"promise_return":560152386,"read_cached_trie_node":2280000000,"read_memory_base":2609863200,"read_memory_byte":3801333,"read_register_base":2517165186,"read_register_byte":98562,"ripemd160_base":853675086,"ripemd160_block":680107584,"sha256_base":4540970250,"sha256_byte":24117351,"storage_has_key_base":54039896625,"storage_has_key_byte":30790845,"storage_iter_create_from_byte":0,"storage_iter_create_prefix_base":0,"storage_iter_create_prefix_byte":0,"storage_iter_create_range_base":0,"storage_iter_create_to_byte":0,"storage_iter_next_base":0,"storage_iter_next_key_byte":0,"storage_iter_next_value_byte":0,"storage_large_read_overhead_base":1,"storage_large_read_overhead_byte":1,"storage_read_base":56356845749,"storage_read_key_byte":30952533,"storage_read_value_byte":5611004,"storage_remove_base":53473030500,"storage_remove_key_byte":38220384,"storage_remove_ret_value_byte":11531556,"storage_write_base":64196736000,"storage_write_evicted_byte":32117307,"storage_write_key_byte":70482867,"storage_write_value_byte":31018539,"touching_trie_node":16101955926,"utf16_decoding_base":3543313050,"utf16_decoding_byte":163577493,"utf8_decoding_base":3111779061,"utf8_decoding_byte":291580479,"validator_stake_base":911834726400,"validator_total_stake_base":911834726400,"write_memory_base":2803794861,"write_memory_byte":2723772,"write_register_base":2865522486,"write_register_byte":3801564,"yield_create_base":153411779276,"yield_create_byte":15643988,"yield_resume_base":1195627285210,"yield_resume_byte":47683715},"fix_contract_loading_cost":false,"global_contract_host_fns":true,"grow_mem_cost":1,"implicit_account_creation":true,"limit_config":{"account_id_validity_rules_version":1,"initial_memory_pages":1024,"max_actions_per_receipt":100,"max_arguments_length":4194304,"max_contract_size":4194304,"max_functions_number_per_contract":10000,"max_gas_burnt":300000000000000,"max_length_method_name":256,"max_length_returned_data":4194304,"max_length_storage_key":2048,"max_length_storage_value":4194304,"max_locals_per_contract":1000000,"max_memory_pages":2048,"max_number_bytes_method_names":2000,"max_number_input_data_dependencies":128,"max_number_logs":100,"max_number_registers":100,"max_promises_per_function_call_action":1024,"max_receipt_size":4194304,"max_register_size":104857600,"max_stack_height":262144,"max_total_log_length":16384,"max_total_prepaid_gas":300000000000000,"max_transaction_size":1572864,"max_yield_payload_size":1024,"per_receipt_storage_proof_size_limit":4000000,"registers_memory_limit":1073741824,"yield_timeout_length_in_blocks":200},"regular_op_cost":822756,"saturating_float_to_int":true,"storage_get_mode":"FlatStorage","vm_kind":"NearVm"},"witness_config":{"combined_transactions_size_limit":4194304,"main_storage_proof_size_soft_limit":4000000,"new_transactions_validation_state_size_soft_limit":572864}},"shard_layout":{"V2":{"boundary_accounts":["650","aurora","aurora-0","earn.kaiching","game.hot.tg","game.hot.tg-0","kkuuue2akv_1630967379.near","tge-lockup.sweat"],"id_to_index_map":{"1":2,"10":0,"11":1,"4":7,"5":8,"6":5,"7":6,"8":3,"9":4},"index_to_id_map":{"0":10,"1":11,"2":1,"3":8,"4":9,"5":6,"6":7,"7":4,"8":5},"shard_ids":[10,11,1,8,9,6,7,4,5],"shards_parent_map":{"1":1,"10":0,"11":0,"4":4,"5":5,"6":6,"7":7,"8":8,"9":9},"shards_split_map":{"0":[10,11],"1":[1],"4":[4],"5":[5],"6":[6],"7":[7],"8":[8],"9":[9]},"version":3}},"shuffle_shard_assignment_for_chunk_producers":false,"target_validator_mandates_per_shard":105,"transaction_validity_period":86400}"#;
        let config: near_chain_configs::ProtocolConfigView =
            near_sdk::serde_json::from_str(config).unwrap();

        assert_eq!(
            action.gas_cost(&receiver_id, sir, &config),
            near_sdk::Gas::from_gas(expected_cost),
        );
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
