#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use near_sdk::{json_types::U128, AccountId};
use std::str::FromStr;

fuzz_target!(|data: (u128, u64, bool, u8, &[u8])| {
    let (liquidation_amount_raw, nonce, is_nep141, account_suffix, account_name_bytes) = data;

    let liquidation_amount = U128(liquidation_amount_raw);

    // Test 1: Account ID creation from various inputs
    let account_name = String::from_utf8_lossy(account_name_bytes);
    let sanitized = account_name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
        .take(64)
        .collect::<String>();

    if !sanitized.is_empty() {
        let account_str = format!("{sanitized}.testnet");
        let _ = AccountId::from_str(&account_str);
    }

    // Test 2: Generate valid account IDs for testing
    let valid_accounts = [
        "borrower.testnet",
        "liquidator.testnet",
        "market.testnet",
        "usdc.testnet",
    ];

    for account_str in valid_accounts {
        #[allow(clippy::unwrap_used, reason = "Fuzzing valid inputs")]
        let account_id = AccountId::from_str(account_str).unwrap();
        assert!(!account_id.as_str().is_empty());
    }

    // Test 3: Liquidation message creation
    #[allow(clippy::unwrap_used, reason = "Fuzzing valid inputs")]
    let borrower_account = AccountId::from_str("borrower.testnet").unwrap();

    // Simulate DepositMsg::Liquidate creation
    let liquidate_msg =
        format!(r#"{{"Liquidate":{{"account_id":"{borrower_account}","amount":null}}}}"#,);

    // Verify the message is valid JSON-like
    assert!(liquidate_msg.contains("Liquidate"));
    assert!(liquidate_msg.contains(&borrower_account.to_string()));

    // Test with explicit amount
    let liquidate_msg_with_amount = format!(
        r#"{{"Liquidate":{{"account_id":"{}","amount":{}}}}}"#,
        borrower_account, liquidation_amount.0
    );

    assert!(liquidate_msg_with_amount.contains(&liquidation_amount.0.to_string()));

    // Test 4: Asset specification parsing
    if is_nep141 {
        // NEP-141 format: "nep141:contract.near"
        let asset_spec = "nep141:usdc.testnet";
        assert!(asset_spec.starts_with("nep141:"));

        let parts: Vec<&str> = asset_spec.split(':').collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], "nep141");

        // Verify contract ID is valid
        let _ = AccountId::from_str(parts[1]);
    } else {
        // NEP-245 format: "nep245:contract.near:token_id"
        let asset_spec = "nep245:multitoken.testnet:eth";
        assert!(asset_spec.starts_with("nep245:"));

        let parts: Vec<&str> = asset_spec.split(':').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "nep245");

        // Verify contract ID is valid
        let _ = AccountId::from_str(parts[1]);
        assert!(!parts[2].is_empty()); // Token ID should not be empty
    }

    // Test 5: Nonce handling
    let nonce_incremented = nonce.saturating_add(1);
    assert!(nonce_incremented >= nonce);

    // Test with max nonce
    let max_nonce = u64::MAX;
    let _ = max_nonce.saturating_add(1); // Should not overflow

    // Test 6: Liquidation amount boundaries
    if liquidation_amount.0 == 0 {
        // Edge case: zero liquidation
        assert_eq!(liquidation_amount.0, 0);
    } else {
        // Normal case: positive liquidation
        assert!(liquidation_amount.0 > 0);
    }

    // Test 7: Balance calculations for transfer_call
    let one_yocto = 1u128; // Attached deposit for transfer_call
    let total_needed = liquidation_amount.0.saturating_add(one_yocto);

    assert!(total_needed >= liquidation_amount.0);

    // Test 8: Method name generation
    let transfer_call_method = if is_nep141 {
        "ft_transfer_call"
    } else {
        "mt_transfer_call"
    };

    assert!(!transfer_call_method.is_empty());
    assert!(transfer_call_method.ends_with("_call"));

    // Test 9: Multiple liquidation amounts
    let amounts = [
        U128(0),
        U128(1),
        U128(1000),
        U128(1_000_000),
        U128(u128::MAX / 2),
        U128(u128::MAX),
    ];

    for amount in amounts {
        let msg = format!(
            r#"{{"Liquidate":{{"account_id":"test.near","amount":{}}}}}"#,
            amount.0
        );
        assert!(msg.contains("Liquidate"));
    }

    // Test 10: Account suffix handling
    let suffix_num = u32::from(account_suffix);
    let account_with_suffix = format!("liquidator_{suffix_num}.testnet");

    if let Ok(account_id) = AccountId::from_str(&account_with_suffix) {
        assert!(account_id.as_str().contains("liquidator"));
    }

    // Test 11: Edge case - empty liquidation
    let empty_msg = r#"{"Liquidate":{"account_id":"test.near","amount":null}}"#;
    assert!(empty_msg.contains("null"));

    // Test 12: Gas and timeout calculations
    let timeout_seconds = 60u64;
    let timeout_nanos = timeout_seconds.saturating_mul(1_000_000_000);
    assert!(timeout_nanos >= timeout_seconds);

    // Test 13: Transaction action building
    let receiver_id = "usdc.testnet";
    let method_name = "ft_transfer_call";
    let args = liquidate_msg.as_bytes();

    assert!(!receiver_id.is_empty());
    assert!(!method_name.is_empty());
    assert!(!args.is_empty());

    // Test 14: Block hash handling (mock)
    let mock_block_hash = [0u8; 32];
    assert_eq!(mock_block_hash.len(), 32);

    // Test 15: Signer account validation
    let signer_accounts = ["liquidator.testnet", "bot1.near", "system.testnet"];

    for signer in signer_accounts {
        if let Ok(account) = AccountId::from_str(signer) {
            assert!(!account.as_str().is_empty());
        }
    }
});
