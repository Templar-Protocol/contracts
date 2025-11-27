#![allow(clippy::unwrap_used)]

use near_sdk::{base64::prelude::*, env::sha256, json_types::U64, NearToken};

use templar_universal_account::{
    authentication::{passkey, with_raw_string::WithRawString, HashForSigning, Payload},
    transaction::{Action, Transaction},
    KeyParameters, PayloadExecutionParameters, NEAR_TESTNET_CHAIN_ID,
};

pub fn main() {
    let payload: Payload<Box<[Transaction]>> = Payload {
        parameters: PayloadExecutionParameters::from_key(
            KeyParameters {
                block_height: U64(123_456),
                index: U64(0),
                nonce: U64(1),
            },
            "default-18843764340.gh-275.templar-in-training.testnet"
                .parse()
                .unwrap(),
        )
        .chain_id(NEAR_TESTNET_CHAIN_ID),
        payload: vec![Transaction {
            receiver_id: "alice.testnet".parse().unwrap(),
            actions: vec![Action::Transfer {
                amount: NearToken::from_near(1),
            }]
            .into(),
        }]
        .into(),
    };
    let payload = passkey::Message(WithRawString::from_parsed(payload));

    let bytes = payload.preimage_for_signing();

    println!("Payload (stringified):");
    println!("{}", String::from_utf8(bytes.clone()).unwrap());
    println!("SHA-256 (base64):");
    println!("{}", BASE64_STANDARD_NO_PAD.encode(sha256(&bytes)));
}
