use near_account_id::AccountId;
use near_sdk::json_types::Base64VecU8;
use templar_common::oracle::redstone;
use templar_gateway_types::{ManagedAccountId, NearToken};

use crate::{
    client::{
        pyth_oracle::UpdatePriceFeedsArgs,
        pyth_pro_oracle::UpdatePriceFeedsArgs as ProUpdatePriceFeedsArgs,
        redstone_oracle::WritePricesArgs, ContractWriteOptions, NearClient,
    },
    GatewayResult, PlannedTransaction,
};

const PYTH_UPDATE_DEPOSIT: NearToken = NearToken::from_yoctonear(10_000_000_000_000_000_000_000);

/// Deposit attached to a Pyth Pro `update_price_feeds` call.
///
/// Unlike classic Pyth (which charges a fixed 0.01 NEAR), the Pyth Pro adapter
/// charges the submitter only for newly consumed storage plus
/// `config.update_fee` (default 0) and refunds any excess. Updates that only
/// overwrite already-stored feeds consume no new storage, so with the default
/// zero fee they are effectively free. The submitter tops up if a particular
/// bundle happens to introduce new feeds; the planner cannot know that ahead of
/// time, so it attaches zero by default.
const PYTH_PRO_UPDATE_DEPOSIT: NearToken = NearToken::from_yoctonear(0);

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

pub fn plan_pyth_pro_update(
    near_client: &NearClient,
    signer_account_id: ManagedAccountId,
    oracle_id: AccountId,
    payload: Vec<u8>,
) -> GatewayResult<PlannedTransaction> {
    near_client.pyth_pro_oracle(oracle_id).update_price_feeds(
        ContractWriteOptions::new(signer_account_id)
            .tgas(300)
            .deposit(PYTH_PRO_UPDATE_DEPOSIT),
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
    fn pro_plan_pyth_pro_update_emits_payload_base64_and_no_data_field() {
        let oracle_id: AccountId = "pyth-pro.near".parse().expect("valid account id");
        let payload = vec![0u8, 1, 2, 3, 255, 254, 253];

        let plan = plan_pyth_pro_update(
            &test_client(),
            signer_id(),
            oracle_id.clone(),
            payload.clone(),
        )
        .expect("pro planner must succeed");

        assert_eq!(plan.receiver_id, oracle_id);
        let (method, args_bytes) = unpack_function_call(&plan);
        assert_eq!(method, "update_price_feeds");

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

    #[test]
    fn pro_plan_pyth_pro_update_with_empty_payload_does_not_panic() {
        let oracle_id: AccountId = "pyth-pro.near".parse().expect("valid account id");

        // Adapter rejects empty on-chain, but the planner is pure serialization
        // and must not panic.
        let plan = plan_pyth_pro_update(&test_client(), signer_id(), oracle_id, vec![])
            .expect("pro planner must not panic on empty payload");

        let (method, args_bytes) = unpack_function_call(&plan);
        assert_eq!(method, "update_price_feeds");

        let args_json: serde_json::Value =
            serde_json::from_slice(&args_bytes).expect("pro empty-payload args must parse");

        let payload_b64 = args_json
            .get("payload")
            .expect("pro args MUST contain `payload` even when empty")
            .as_str()
            .expect("`payload` must be a json string");
        assert!(
            payload_b64.is_empty(),
            "base64 of empty bytes must be the empty string; got {payload_b64:?}"
        );
        assert!(
            args_json.get("data").is_none(),
            "pro empty-payload args MUST NOT carry a `data` field"
        );
    }
}
