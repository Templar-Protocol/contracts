#![allow(clippy::unwrap_used)]

use near_sdk::{base64::prelude::*, env::sha256, json_types::U64, serde_json, NearToken};
use templar_universal_account_contract::transaction::{Action, Transaction};

pub fn main() {
    let transaction = Transaction {
        receiver_id: "alice.testnet".parse().unwrap(),
        nonce: U64(1),
        actions: vec![Action::Transfer {
            amount: NearToken::from_near(1),
        }],
    };

    let s = serde_json::to_string(&transaction).unwrap();
    println!("Payload:");
    println!("{s}");
    println!("SHA-256 (base64):");
    println!("{}", BASE64_STANDARD_NO_PAD.encode(sha256(s.as_bytes())));
}
