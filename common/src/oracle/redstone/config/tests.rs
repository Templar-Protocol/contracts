use hex_literal::hex;
use near_sdk::{borsh, env, serde_json};
use redstone::{Crypto, CryptoError, SignerAddress};

use super::{config_prod, Config, ConfigValidationError, NearCrypto, SignerAddressBs};

/// One data package of the `adapter::tests::payload` fixture: the bytes a
/// production RedStone signer signed for feed `ETH`, and its signature.
const MESSAGE: [u8; 77] = hex!(
    "4554480000000000000000000000000000000000000000000000000000000000"
    "0000000000000000000000000000000000000000000000000000002d9030a710"
    "019c56f0bec000000020000001"
);
const SIGNATURE: [u8; 65] = hex!(
    "5d1cb1a708c63264741b00ce097176e45f708914b8cfdca26b079877a70604e2"
    "5aa0bcfa3a41df8212eddd51db3496b95c7c3dc4caa9ac9705602af0515db1b3"
    "1c"
);
/// `SIGNATURE` with `s` replaced by `n - s` and the recovery byte flipped: as
/// valid for the same signer and message, in the form ECDSA malleability allows.
const HIGH_S_SIGNATURE: [u8; 65] = hex!(
    "5d1cb1a708c63264741b00ce097176e45f708914b8cfdca26b079877a70604e2"
    "a55f4305c5be207ded1222ae24cb69455e329f21e49ef3a4ba72339c7ed88f8e"
    "1b"
);

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

#[test]
fn recovers_the_key_and_signer_of_a_redstone_package() {
    let key = NearCrypto
        .recover_public_key(1, &SIGNATURE[..64], env::keccak256_array(MESSAGE))
        .unwrap();
    assert_eq!(
        key.as_ref(),
        hex!(
            "04"
            "683f7100d67fbaccae68db4d146f7bc3090c5cbcb1351e333efd58d42f609cee"
            "ada12b0af28a6e789d6edfe25fe784dd1e86798240d4cee9f60e56848cb55965"
        )
        .as_slice()
    );
    assert_eq!(
        NearCrypto.recover_address(MESSAGE, SIGNATURE),
        Ok(SignerAddress::from(
            config_prod::ALLOWED_SIGNERS[1].to_vec()
        ))
    );
}

#[test]
fn refuses_a_high_s_signature_in_the_sdk_and_in_the_host() {
    // The SDK's own check answers `Signature`; the host's refusal is `RecoverPreHash`.
    assert!(matches!(
        NearCrypto.recover_address(MESSAGE, HIGH_S_SIGNATURE),
        Err(CryptoError::Signature(_))
    ));
    assert_eq!(
        NearCrypto
            .recover_public_key(0, &HIGH_S_SIGNATURE[..64], env::keccak256_array(MESSAGE))
            .err(),
        Some(CryptoError::RecoverPreHash)
    );
}

#[rstest::rstest]
#[case::short_signature(1, &SIGNATURE[..63], CryptoError::Signature(SIGNATURE[..63].to_vec()))]
#[case::long_signature(1, &SIGNATURE, CryptoError::Signature(SIGNATURE.to_vec()))]
#[case::recovery_byte_4(4, &SIGNATURE[..64], CryptoError::RecoveryByte(4))]
#[case::recovery_byte_2(2, &SIGNATURE[..64], CryptoError::RecoverPreHash)]
#[case::recovery_byte_3(3, &SIGNATURE[..64], CryptoError::RecoverPreHash)]
fn reports_an_error_where_the_host_aborts_or_recovers_nothing(
    #[case] recovery_byte: u8,
    #[case] signature: &[u8],
    #[case] error: CryptoError,
) {
    assert_eq!(
        NearCrypto
            .recover_public_key(recovery_byte, signature, env::keccak256_array(MESSAGE))
            .err(),
        Some(error)
    );
}
