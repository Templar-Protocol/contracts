#![allow(clippy::unwrap_used)]

use near_sdk::serde_json::{self, json};
use templar_universal_account_contract::{
    authentication::passkey::Message, key::p256::PublicKey, KeyId,
};

pub fn main() {
    let message: Message = serde_json::from_value(json!({
        "authenticator_data": "49960de5880e8c687434170f6476605b8fe4aeb9a28632c7995cf3ba831d97631d00000000",
        "payload": r#"{"receiver_id":"alice.testnet","nonce":"1","actions":[{"Transfer":{"amount":"1000000000000000000000000"}}]}"#,
        "client_data_json": r#"{"type":"webauthn.get","challenge":"85VxczPqag4d7XrZFvpPZBBuac_cLiQZGONVSkdl9LE","origin":"http://localhost:3000","crossOrigin":false}"#,
        "signature": "MEUCICy0TG2AuV8mOv-HEsTayGBiA4huWNJ5sUKzsQWt1xwnAiEA5_lulYfwRnf9dPSHBNciq63jrIFx0LAB519gQuJGxU8",
    })).unwrap();

    let passkey: PublicKey = "p256:S8avjv5zYFYhViXo7giqwynnMdox3RAytXQ7FG9a2tj8WxZnU6KUr36MSuUvgrwk4uGNMdiXt6vwtL9yBvj6VAUL".parse().unwrap();

    let args = json!({
        "key": KeyId::Passkey(passkey),
        "message": message,
    });

    println!("{}", serde_json::to_string(&args).unwrap());
}
