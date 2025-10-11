#![allow(clippy::unwrap_used)]

use near_sdk::{base64::prelude::*, env::sha256, json_types::U64, serde_json, NearToken};

use templar_universal_account::{
    authentication::passkey::Payload,
    transaction::{Action, Transaction},
    ExecutionParameters,
};

pub fn main() {
    let payload: Payload<Box<[Transaction]>> = Payload {
        parameters: ExecutionParameters {
            index: U64(0),
            nonce: U64(1),
        },
        account_id: "my-universal-account.testnet".parse().unwrap(),
        payload: vec![Transaction {
            receiver_id: "alice.testnet".parse().unwrap(),
            actions: vec![Action::Transfer {
                amount: NearToken::from_near(1),
            }]
            .into(),
        }]
        .into(),
    };

    let s = serde_json::to_string(&payload).unwrap();
    println!("Payload:");
    println!("{s}");
    println!("SHA-256 (base64):");
    println!("{}", BASE64_STANDARD_NO_PAD.encode(sha256(s.as_bytes())));
}
