use near_sdk::{borsh, serde_json};

use super::{Config, ConfigValidationError, SignerAddressBs};

fn signers(count: u16) -> Vec<SignerAddressBs> {
    (1..=count)
        .map(|index| {
            let mut address = [0; 20];
            address[..2].copy_from_slice(&index.to_le_bytes());
            address
        })
        .collect()
}

fn config() -> Config {
    Config {
        signer_count_threshold: 3,
        signers: signers(5),
        max_timestamp_delay_ms: 100,
        max_timestamp_ahead_ms: 200,
        min_interval_between_updates_ms: 300,
    }
}

#[test]
fn signer_policy_bounds() {
    for (signers, threshold) in [(signers(1), 1), (signers(255), 255)] {
        let candidate = Config {
            signers,
            signer_count_threshold: threshold,
            ..config()
        };
        candidate.validate().unwrap();
    }
    for (signers, threshold) in [
        (signers(0), 1),
        (signers(256), 1),
        (signers(1), 2),
        (vec![[1; 20], [1; 20]], 1),
        (vec![[0; 20]], 1),
    ] {
        assert!(matches!(
            Config {
                signers,
                signer_count_threshold: threshold,
                ..config()
            }
            .validate(),
            Err(ConfigValidationError::InvalidSigners(_))
        ));
    }
    assert!(matches!(
        Config {
            signer_count_threshold: 0,
            ..config()
        }
        .validate(),
        Err(ConfigValidationError::ZeroSignerThreshold)
    ));
}

#[test]
fn timing_fields_accept_the_full_u64_range() {
    for timing in [0, u64::MAX] {
        Config {
            max_timestamp_delay_ms: timing,
            max_timestamp_ahead_ms: timing,
            min_interval_between_updates_ms: timing,
            ..config()
        }
        .validate()
        .unwrap();
    }
}

#[test]
fn historical_invalid_config_wire_forms_remain_readable() {
    let historical = Config {
        signer_count_threshold: 0,
        signers: vec![],
        max_timestamp_delay_ms: 0,
        max_timestamp_ahead_ms: u64::MAX,
        min_interval_between_updates_ms: 7,
    };
    let json = serde_json::json!({
        "signer_count_threshold": 0,
        "signers": [],
        "max_timestamp_delay_ms": 0,
        "max_timestamp_ahead_ms": u64::MAX,
        "min_interval_between_updates_ms": 7,
    });
    assert_eq!(serde_json::to_value(&historical).unwrap(), json);
    let decoded: Config = serde_json::from_value(json).unwrap();
    assert_eq!(decoded, historical);
    let mut bytes = vec![0, 0, 0, 0, 0];
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    bytes.extend_from_slice(&u64::MAX.to_le_bytes());
    bytes.extend_from_slice(&7_u64.to_le_bytes());
    assert_eq!(borsh::to_vec(&historical).unwrap(), bytes);
    assert_eq!(borsh::from_slice::<Config>(&bytes).unwrap(), historical);
    assert!(decoded.validate().is_err());
}

#[test]
fn valid_config_preserves_json_and_borsh_wire_forms() {
    let config = config();
    let json = serde_json::json!({
        "signer_count_threshold": 3,
        "signers": signers(5),
        "max_timestamp_delay_ms": 100,
        "max_timestamp_ahead_ms": 200,
        "min_interval_between_updates_ms": 300,
    });
    assert_eq!(serde_json::to_value(&config).unwrap(), json);
    assert_eq!(serde_json::from_value::<Config>(json).unwrap(), config);
    let mut bytes = vec![3];
    bytes.extend_from_slice(&5_u32.to_le_bytes());
    for signer in &config.signers {
        bytes.extend_from_slice(signer);
    }
    for timing in [100_u64, 200, 300] {
        bytes.extend_from_slice(&timing.to_le_bytes());
    }
    assert_eq!(borsh::to_vec(&config).unwrap(), bytes);
    assert_eq!(borsh::from_slice::<Config>(&bytes).unwrap(), config);
}
