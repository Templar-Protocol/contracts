#![allow(clippy::unwrap_used)]

use near_sdk::serde_json;

use templar_universal_account::{
    authentication::passkey::Passkey, encoding::p256::PublicKey, InitArgs, KeyId,
    NEAR_TESTNET_CHAIN_ID,
};

pub fn main() {
    // Replace with your own public key.
    // let public_key: PublicKey = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng).public_key().into();
    let public_key: PublicKey = "p256:S8avjv5zYFYhViXo7giqwynnMdox3RAytXQ7FG9a2tj8WxZnU6KUr36MSuUvgrwk4uGNMdiXt6vwtL9yBvj6VAUL".parse().unwrap();

    let init = InitArgs {
        key: KeyId::Passkey(Passkey(public_key)),
        chain_id: NEAR_TESTNET_CHAIN_ID.into(),
    };

    println!("{}", serde_json::to_string(&init).unwrap());
}
