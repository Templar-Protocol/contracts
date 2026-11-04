use near_sdk::{borsh, serde::Deserialize, serde_json};

use super::{Config, ConfigPatch, ConfigValidationError, SignerAddressBs};

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

fn patch(json: &str) -> ConfigPatch {
    serde_json::from_str(json).unwrap()
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
        let mut current = config();
        let before = current.clone();
        assert!(matches!(
            current.apply_patch(ConfigPatch {
                signers: Some(signers.clone()),
                signer_count_threshold: Some(threshold),
                max_timestamp_delay_ms: Some(0),
                ..ConfigPatch::default()
            }),
            Err(ConfigValidationError::InvalidSigners(_))
        ));
        assert_eq!(current, before);
        assert!(Config {
            signers,
            signer_count_threshold: threshold,
            ..before
        }
        .validate()
        .is_err());
    }
    let mut current = config();
    let before = current.clone();
    let error = current
        .apply_patch(ConfigPatch {
            signer_count_threshold: Some(0),
            signers: Some(vec![]),
            ..ConfigPatch::default()
        })
        .unwrap_err();
    assert!(matches!(error, ConfigValidationError::ZeroSignerThreshold));
    assert_eq!(
        error.to_string(),
        "signer_count_threshold must be greater than zero"
    );
    assert_eq!(current, before);
}

#[test]
fn independent_policy_fields_preserve_the_other_fields() {
    let mut current = config();
    let mut replacement = signers(4);
    replacement.reverse();
    current
        .apply_patch(ConfigPatch {
            signers: Some(replacement.clone()),
            ..ConfigPatch::default()
        })
        .unwrap();
    assert_eq!(current.signers, replacement);
    assert_eq!(current.signer_count_threshold, 3);
    current
        .apply_patch(ConfigPatch {
            signer_count_threshold: Some(4),
            ..ConfigPatch::default()
        })
        .unwrap();
    assert_eq!(current.signers, replacement);
    assert_eq!(current.signer_count_threshold, 4);
    let before = current.clone();
    current
        .apply_patch(patch(r#"{"max_timestamp_delay_ms":7}"#))
        .unwrap();
    assert_eq!(
        current,
        Config {
            max_timestamp_delay_ms: 7,
            ..before
        }
    );
    current.signer_count_threshold = 0;
    assert!(matches!(
        current.validate(),
        Err(ConfigValidationError::ZeroSignerThreshold)
    ));
}
#[test]
fn partial_null_zero_and_same_value_updates() {
    let mut current = config();
    let original = current.clone();
    current
        .apply_patch(patch(r#"{"max_timestamp_delay_ms":0,"signers":null}"#))
        .unwrap();
    assert_eq!(
        current,
        Config {
            max_timestamp_delay_ms: 0,
            ..original
        }
    );
    current
        .apply_patch(patch(r#"{"max_timestamp_delay_ms":0}"#))
        .unwrap();
    current
        .apply_patch(ConfigPatch {
            signer_count_threshold: Some(1),
            signers: Some(signers(1)),
            max_timestamp_delay_ms: Some(u64::MAX),
            max_timestamp_ahead_ms: Some(0),
            min_interval_between_updates_ms: Some(u64::MAX),
        })
        .unwrap();
    assert_eq!(
        current,
        Config {
            signer_count_threshold: 1,
            signers: signers(1),
            max_timestamp_delay_ms: u64::MAX,
            max_timestamp_ahead_ms: 0,
            min_interval_between_updates_ms: u64::MAX,
        }
    );
    current
        .apply_patch(ConfigPatch {
            max_timestamp_delay_ms: Some(0),
            max_timestamp_ahead_ms: Some(u64::MAX),
            min_interval_between_updates_ms: Some(0),
            ..ConfigPatch::default()
        })
        .unwrap();
    assert_eq!(current.max_timestamp_delay_ms, 0);
    assert_eq!(current.max_timestamp_ahead_ms, u64::MAX);
    assert_eq!(current.min_interval_between_updates_ms, 0);
}

#[test]
fn empty_and_malformed_patches() {
    #[derive(Deserialize)]
    #[serde(crate = "near_sdk::serde")]
    struct Args {
        patch: ConfigPatch,
    }
    let mut current = config();
    let before = current.clone();
    for json in [
        "{}",
        r#"{"signer_count_threshold":null,"signers":null,"max_timestamp_delay_ms":null,"max_timestamp_ahead_ms":null,"min_interval_between_updates_ms":null}"#,
    ] {
        let error = current.apply_patch(patch(json)).unwrap_err();
        assert!(matches!(error, ConfigValidationError::EmptyPatch));
        assert_eq!(
            error.to_string(),
            "configuration patch must supply at least one field"
        );
        assert_eq!(current, before);
    }
    for json in [
        r#"{"unknown":1}"#,
        r#"{"signer_count_threshold":"1"}"#,
        r#"{"signers":[] ,"max_timestamp_delay_ms":false}"#,
        "null",
        "[]",
        "[null,null,null,null,null]",
        "[1,[],0,0,0]",
    ] {
        assert!(serde_json::from_str::<ConfigPatch>(json).is_err());
    }
    for json in ["{}", r#"{"patch":null}"#] {
        assert!(serde_json::from_str::<Args>(json).is_err());
    }
    assert_eq!(
        serde_json::from_str::<Args>(r#"{"patch":{}}"#)
            .unwrap()
            .patch,
        ConfigPatch::default()
    );
}

#[test]
fn coupled_policy_is_atomic_and_uses_execution_time_state() {
    let mut current = config();
    let before = current.clone();
    assert!(current
        .apply_patch(ConfigPatch {
            signers: Some(signers(1)),
            ..ConfigPatch::default()
        })
        .is_err());
    assert_eq!(current, before);
    assert!(current
        .apply_patch(ConfigPatch {
            signer_count_threshold: Some(6),
            max_timestamp_delay_ms: Some(0),
            ..ConfigPatch::default()
        })
        .is_err());
    assert_eq!(current, before);
    let queued = ConfigPatch {
        signers: Some(signers(3)),
        ..ConfigPatch::default()
    };
    let mut preview = current.clone();
    preview.apply_patch(queued.clone()).unwrap();
    current
        .apply_patch(ConfigPatch {
            signer_count_threshold: Some(5),
            ..ConfigPatch::default()
        })
        .unwrap();
    let before_execution = current.clone();
    assert!(current.apply_patch(queued).is_err());
    assert_eq!(current, before_execution);
    current
        .apply_patch(ConfigPatch {
            signer_count_threshold: Some(1),
            signers: Some(signers(1)),
            ..ConfigPatch::default()
        })
        .unwrap();
    current
        .apply_patch(patch(r#"{"max_timestamp_ahead_ms":60000}"#))
        .unwrap();
    current
        .apply_patch(patch(r#"{"max_timestamp_delay_ms":120000}"#))
        .unwrap();
    assert_eq!(current.max_timestamp_ahead_ms, 60_000);
    assert_eq!(current.max_timestamp_delay_ms, 120_000);
    assert_eq!(
        current.min_interval_between_updates_ms,
        before.min_interval_between_updates_ms
    );
}

#[test]
fn historical_config_wire_forms_remain_readable_and_repairable() {
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
    let mut decoded: Config = serde_json::from_value(json).unwrap();
    assert_eq!(decoded, historical);
    let mut bytes = vec![0, 0, 0, 0, 0];
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    bytes.extend_from_slice(&u64::MAX.to_le_bytes());
    bytes.extend_from_slice(&7_u64.to_le_bytes());
    assert_eq!(borsh::to_vec(&historical).unwrap(), bytes);
    assert_eq!(borsh::from_slice::<Config>(&bytes).unwrap(), historical);
    assert!(decoded.validate().is_err());
    decoded
        .apply_patch(ConfigPatch {
            signers: Some(signers(1)),
            signer_count_threshold: Some(1),
            ..ConfigPatch::default()
        })
        .unwrap();
    decoded.validate().unwrap();
    assert_eq!(decoded.max_timestamp_ahead_ms, u64::MAX);
    assert_eq!(decoded.min_interval_between_updates_ms, 7);
    let encoded = serde_json::to_value(&decoded).unwrap();
    assert_eq!(encoded["signers"], serde_json::json!(signers(1)));
    assert_eq!(
        borsh::from_slice::<Config>(&borsh::to_vec(&decoded).unwrap()).unwrap(),
        decoded
    );
}
