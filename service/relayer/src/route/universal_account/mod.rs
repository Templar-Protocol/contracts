use sha2::{Digest, Sha256};

pub mod create;
pub mod pow;

pub fn public_key_to_account_id_slug(public_key: &str) -> String {
    hex::encode(&Sha256::digest(public_key)[0..12])
}
