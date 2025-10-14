#![allow(clippy::unwrap_used)]

use near_sdk::{base64::prelude::*, env::sha256, json_types::U64, NearToken};

use templar_universal_account::{
    authentication::passkey::{with_raw_string::WithRawString, Payload},
    transaction::{Action, Transaction},
    ExecutionParameters,
};

pub fn main() {
    let payload: Payload<Box<[Transaction]>> = Payload {
        parameters: ExecutionParameters {
            block_height: U64(123_456),
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
    let payload = WithRawString::from_parsed(payload);

    let bytes = payload.bytes_with_magic_number();

    println!("Payload (stringified):");
    println!("{}", String::from_utf8(bytes.clone()).unwrap());
    println!("SHA-256 (base64):");
    println!("{}", BASE64_STANDARD_NO_PAD.encode(sha256(&bytes)));
}
