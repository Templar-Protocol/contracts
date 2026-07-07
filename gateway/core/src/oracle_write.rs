use near_account_id::AccountId;
use near_sdk::json_types::Base64VecU8;
use templar_common::oracle::redstone;
use templar_gateway_types::{ManagedAccountId, NearToken};

use crate::{
    client::{
        pyth_lazer_oracle::UpdatePriceFeedsArgs as ProUpdatePriceFeedsArgs,
        pyth_oracle::UpdatePriceFeedsArgs, redstone_oracle::WritePricesArgs, ContractWriteOptions,
        NearClient,
    },
    GatewayResult, PlannedTransaction,
};

const PYTH_UPDATE_DEPOSIT: NearToken = NearToken::from_yoctonear(10_000_000_000_000_000_000_000);

/// Deposit attached to a Pyth Lazer `update_price_feeds` call.
///
/// The adapter charges the submitter (this gateway) for any newly consumed
/// storage plus `config.update_fee`, and `apply_storage_fee_and_refund`
/// *reverts* the call if the attached deposit does not cover that cost. A first
/// update for a feed the adapter hasn't stored yet, or any adapter configured
/// with a non-zero fee, therefore requires a non-zero deposit — so we attach a
/// conservative fixed amount (matching classic Pyth) and let the adapter refund
/// the excess. Overwrite-only updates consume no new storage, so with the
/// default zero fee the whole deposit is refunded and they cost only gas.
const PYTH_LAZER_UPDATE_DEPOSIT: NearToken =
    NearToken::from_yoctonear(10_000_000_000_000_000_000_000);

pub fn plan_pyth_update(
    near_client: &NearClient,
    signer_account_id: ManagedAccountId,
    oracle_id: AccountId,
    vaa: Vec<u8>,
) -> GatewayResult<PlannedTransaction> {
    near_client.pyth_oracle(oracle_id).update_price_feeds(
        ContractWriteOptions::new(signer_account_id)
            .tgas(300)
            .deposit(PYTH_UPDATE_DEPOSIT),
        UpdatePriceFeedsArgs {
            data: hex::encode(vaa),
        },
    )
}

pub fn plan_pyth_lazer_update(
    near_client: &NearClient,
    signer_account_id: ManagedAccountId,
    oracle_id: AccountId,
    payload: Vec<u8>,
) -> GatewayResult<PlannedTransaction> {
    near_client.pyth_lazer_oracle(oracle_id).update_price_feeds(
        ContractWriteOptions::new(signer_account_id)
            .tgas(300)
            .deposit(PYTH_LAZER_UPDATE_DEPOSIT),
        ProUpdatePriceFeedsArgs {
            payload: Base64VecU8(payload),
        },
    )
}

pub fn plan_redstone_write_prices(
    near_client: &NearClient,
    signer_account_id: ManagedAccountId,
    oracle_id: AccountId,
    feed_ids: Vec<redstone::FeedId>,
    payload: Vec<u8>,
) -> GatewayResult<PlannedTransaction> {
    near_client.redstone_oracle(oracle_id).write_prices(
        ContractWriteOptions::new(signer_account_id).tgas(300),
        WritePricesArgs {
            feed_ids,
            payload: Base64VecU8(payload),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use near_api::types::transaction::actions::Action;
    use near_api::NetworkConfig;

    fn test_client() -> NearClient {
        NearClient::new(NetworkConfig::from_rpc_url(
            "test",
            "https://example.test".parse().expect("valid url"),
        ))
    }

    fn signer_id() -> ManagedAccountId {
        ManagedAccountId("relayer.near".parse().expect("valid account id"))
    }

    /// Pull the single function-call action out of a plan: `(method, args)`.
    /// The single-action shape is part of the contract being pinned.
    fn unpack_function_call(plan: &PlannedTransaction) -> (String, Vec<u8>) {
        assert_eq!(
            plan.actions.len(),
            1,
            "planner must produce exactly one action, got {}",
            plan.actions.len()
        );
        match &plan.actions[0] {
            Action::FunctionCall(fc) => (fc.method_name.clone(), fc.args.clone()),
            other => panic!("expected FunctionCall action, got {other:?}"),
        }
    }

    #[derive(serde::Deserialize)]
    struct ClassicArgs {
        data: String,
    }

    #[derive(serde::Deserialize)]
    struct ProArgs {
        payload: Base64VecU8,
    }

    #[test]
    fn classic_plan_pyth_update_emits_data_hex_and_no_payload_field() {
        let oracle_id: AccountId = "pyth-oracle.near".parse().expect("valid account id");
        let vaa = vec![0u8, 1, 2, 3, 255, 254, 253];

        let plan = plan_pyth_update(&test_client(), signer_id(), oracle_id.clone(), vaa.clone())
            .expect("classic planner must succeed");

        assert_eq!(plan.receiver_id, oracle_id);
        let (method, args_bytes) = unpack_function_call(&plan);
        assert_eq!(method, "update_price_feeds");

        let args_str = String::from_utf8(args_bytes).expect("classic args must be utf8 json");
        let args_json: serde_json::Value =
            serde_json::from_str(&args_str).expect("classic args must parse");

        let classic: ClassicArgs = serde_json::from_str(&args_str)
            .expect("classic args must deserialize into {data: String}");
        assert_eq!(classic.data, hex::encode(&vaa));

        assert!(
            args_json.get("payload").is_none(),
            "classic args MUST NOT carry a `payload` field; got: {args_str}"
        );
    }

    #[test]
    fn pro_plan_pyth_lazer_update_emits_payload_base64_and_no_data_field() {
        let oracle_id: AccountId = "pyth-lazer.near".parse().expect("valid account id");
        let payload = vec![0u8, 1, 2, 3, 255, 254, 253];

        let plan = plan_pyth_lazer_update(
            &test_client(),
            signer_id(),
            oracle_id.clone(),
            payload.clone(),
        )
        .expect("pro planner must succeed");

        assert_eq!(plan.receiver_id, oracle_id);
        let (method, args_bytes) = unpack_function_call(&plan);
        assert_eq!(method, "update_price_feeds");

        // A first-time feed write (new storage) or a fee-configured adapter reverts without a
        // deposit, since the adapter charges storage + update_fee and refunds the excess.
        match &plan.actions[0] {
            Action::FunctionCall(fc) => assert!(
                fc.deposit.as_yoctonear() > 0,
                "pro update must attach a non-zero storage/fee deposit"
            ),
            other => panic!("expected FunctionCall action, got {other:?}"),
        }

        let args_str = String::from_utf8(args_bytes).expect("pro args must be utf8 json");
        let args_json: serde_json::Value =
            serde_json::from_str(&args_str).expect("pro args must parse");

        // Deserialize via `Base64VecU8` — the same type the adapter uses — so
        // the round-trip is the actual on-chain wire shape, not a re-derivation.
        let pro: ProArgs = serde_json::from_str(&args_str)
            .expect("pro args must deserialize into {payload: Base64VecU8}");
        assert_eq!(pro.payload.0, payload);

        assert!(
            args_json.get("data").is_none(),
            "pro args MUST NOT carry a `data` field; got: {args_str}"
        );
    }
}
