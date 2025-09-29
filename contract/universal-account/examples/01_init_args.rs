#![allow(clippy::unwrap_used)]

use near_sdk::{json_types::U64, serde_json};

use templar_universal_account::{authentication::passkey::Passkey, key::p256::PublicKey, KeyId};

pub fn main() {
    // Replace with your own public key.
    // let public_key: PublicKey = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng).public_key().into();
    let public_key: PublicKey = "p256:S8avjv5zYFYhViXo7giqwynnMdox3RAytXQ7FG9a2tj8WxZnU6KUr36MSuUvgrwk4uGNMdiXt6vwtL9yBvj6VAUL".parse().unwrap();

    let init = serde_json::json!({
        "key": KeyId::Passkey(Passkey(public_key)),
        "nonce": U64(0),
    });

    println!("{}", serde_json::to_string(&init).unwrap());
}
