#![allow(clippy::unwrap_used)]

use std::str::FromStr;

use near_sdk::serde_json::{self, json};

use templar_universal_account::{
    authentication::passkey::{data::AuthenticatorData, Passkey, UncheckedMessage},
    encoding::p256::PublicKey,
    transaction::Transaction,
    KeyId,
};

pub fn main() {
    let message: UncheckedMessage<Vec<Transaction>> = UncheckedMessage {
        authenticator_data: AuthenticatorData::from_str(
            "49960de5880e8c687434170f6476605b8fe4aeb9a28632c7995cf3ba831d97631d00000000",
        )
        .unwrap(),
        message: r#"{"parameters":{"block_height":"123456","index":"0","nonce":"1"},"account_id":"my-universal-account.testnet","payload":[{"receiver_id":"alice.testnet","actions":[{"Transfer":{"amount":"1000000000000000000000000"}}]}]}"#.parse().unwrap(),
        client_data_json: r#"{"type":"webauthn.get","challenge":"85VxczPqag4d7XrZFvpPZBBuac_cLiQZGONVSkdl9LE","origin":"http://localhost:3000","crossOrigin":false}"#.parse().unwrap(),
        signature: "MEUCICy0TG2AuV8mOv-HEsTayGBiA4huWNJ5sUKzsQWt1xwnAiEA5_lulYfwRnf9dPSHBNciq63jrIFx0LAB519gQuJGxU8".parse().unwrap(),
    };

    let passkey: PublicKey = "p256:S8avjv5zYFYhViXo7giqwynnMdox3RAytXQ7FG9a2tj8WxZnU6KUr36MSuUvgrwk4uGNMdiXt6vwtL9yBvj6VAUL".parse().unwrap();

    let args = json!({
        "key": KeyId::Passkey(Passkey(passkey)),
        "message": message,
    });

    println!("{}", serde_json::to_string(&args).unwrap());
}
