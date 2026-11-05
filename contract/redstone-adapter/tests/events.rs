use near_sdk::{
    json_types::U64,
    serde_json::{self, json, Value},
    test_utils::{get_logs, VMContextBuilder},
    testing_env, NearToken,
};
use templar_common::oracle::redstone::{Config, ConfigPatch, RedStoneContractInterface};
use templar_redstone_adapter_contract::Contract;

#[allow(
    clippy::unwrap_used,
    reason = "invalid test account literals must fail immediately"
)]
fn context(caller: &str, deposit: u128) {
    testing_env!(VMContextBuilder::new()
        .current_account_id("adapter.near".parse().unwrap())
        .predecessor_account_id(caller.parse().unwrap())
        .attached_deposit(NearToken::from_yoctonear(deposit))
        .build());
}

fn config() -> Config {
    Config {
        signer_count_threshold: 1,
        signers: vec![[1; 20], [0xfe; 20]],
        max_timestamp_delay_ms: 10,
        max_timestamp_ahead_ms: 20,
        min_interval_between_updates_ms: 30,
    }
}

#[test]
fn event_records_complete_effective_policy_and_signer_rotation() {
    context("owner.near", 0);
    let mut adapter = Contract::new(config(), "owner.near".parse().unwrap());
    context("owner.near", 1);
    adapter.admin_update_config(ConfigPatch {
        max_timestamp_delay_ms: Some(0),
        ..ConfigPatch::default()
    });
    let expected = r#"EVENT_JSON:{"standard":"redstone-adapter","version":"1.0.0","event":"config_updated","data":{"updater":"owner.near","config":{"signer_count_threshold":1,"signers":["0101010101010101010101010101010101010101","fefefefefefefefefefefefefefefefefefefefe"],"max_timestamp_delay_ms":0,"max_timestamp_ahead_ms":20,"min_interval_between_updates_ms":30}}}"#;
    assert_eq!(get_logs(), vec![expected]);

    // Successful same-value calls still leave an audit record.
    context("owner.near", 1);
    adapter.admin_update_config(ConfigPatch {
        max_timestamp_delay_ms: Some(0),
        ..ConfigPatch::default()
    });
    assert_eq!(get_logs(), vec![expected]);

    context("owner.near", 1);
    adapter.admin_update_config(ConfigPatch {
        signers: Some(vec![[0xab; 20]]),
        ..ConfigPatch::default()
    });
    let logs = get_logs();
    let event: Value = serde_json::from_str(logs[0].strip_prefix("EVENT_JSON:").unwrap()).unwrap();
    assert_eq!(
        event,
        json!({
            "standard": "redstone-adapter", "version": "1.0.0", "event": "config_updated",
            "data": {"updater": "owner.near", "config": {
                "signer_count_threshold": 1,
                "signers": ["abababababababababababababababababababab"],
                "max_timestamp_delay_ms": 0,
                "max_timestamp_ahead_ms": 20,
                "min_interval_between_updates_ms": 30,
            }}
        })
    );
    assert_eq!(adapter.get_config().signers, vec![[0xab; 20]]);
    assert_eq!(adapter.unique_signer_threshold(), U64(1));
}

#[rstest::rstest]
#[case::invalid_policy("owner.near", 1, Some(0))]
#[case::unauthorized("stranger.near", 1, Some(1))]
#[case::wrong_deposit("owner.near", 0, Some(1))]
#[case::empty_patch("owner.near", 1, None)]
fn rejected_patch_never_advertises_a_configuration_update(
    #[case] caller: &str,
    #[case] deposit: u128,
    #[case] threshold: Option<u8>,
) {
    context("owner.near", 0);
    let mut adapter = Contract::new(config(), "owner.near".parse().unwrap());
    context(caller, deposit);
    let before = adapter.get_config().clone();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        adapter.admin_update_config(ConfigPatch {
            signer_count_threshold: threshold,
            ..ConfigPatch::default()
        });
    }));
    assert!(result.is_err());
    assert_eq!(adapter.get_config(), &before);
    assert!(!get_logs()
        .iter()
        .any(|log| log.contains(r#""event":"config_updated""#)));
}

#[test]
fn maximum_signer_policy_emits_losslessly_within_receipt_log_budget() {
    let owner = "o".repeat(64);
    context(&owner, 0);
    let mut adapter = Contract::new(config(), owner.parse().unwrap());
    let signers: Vec<_> = (0..u8::MAX)
        .map(|index| {
            let mut signer = [0xff; 20];
            signer[19] = index;
            signer
        })
        .collect();
    context(&owner, 1);
    adapter.admin_update_config(ConfigPatch {
        signer_count_threshold: Some(u8::MAX),
        signers: Some(signers.clone()),
        max_timestamp_delay_ms: Some(u64::MAX),
        max_timestamp_ahead_ms: Some(u64::MAX),
        min_interval_between_updates_ms: Some(u64::MAX),
    });
    let logs = get_logs();
    assert!(logs.iter().map(String::len).sum::<usize>() < 16 * 1024);
    let event: Value = serde_json::from_str(logs[0].strip_prefix("EVENT_JSON:").unwrap()).unwrap();
    assert_eq!(
        event["data"],
        json!({"updater": owner, "config": {
            "signer_count_threshold": 255,
            "signers": signers.iter().map(hex_address).collect::<Vec<_>>(),
            "max_timestamp_delay_ms": u64::MAX,
            "max_timestamp_ahead_ms": u64::MAX,
            "min_interval_between_updates_ms": u64::MAX,
        }})
    );
    assert_eq!(adapter.get_config().signers, signers);
}

fn hex_address(address: &[u8; 20]) -> String {
    format!("{:040x}", primitive_types::U256::from_big_endian(address))
}
