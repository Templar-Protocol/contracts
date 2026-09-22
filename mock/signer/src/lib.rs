//! Stand-in for the NEAR MPC signer (`v1.signer`), ed25519 only: keys are
//! derived from `(predecessor, path, domain_id)` and `sign` signs in-contract.
//! `set_refuse` makes `sign` panic, as when the MPC network does not answer.

// `#[near]` method parameters are taken by value; the generated wrappers own them.
#![allow(clippy::needless_pass_by_value)]

use ed25519_dalek::{Signer as _, SigningKey};
use near_sdk::{env, near, AccountId, CurveType, NearToken, PanicOnDefault, PublicKey};
use sha2::{Digest as _, Sha256};

const ED25519_DOMAIN_ID: u64 = 1;

#[derive(PanicOnDefault)]
#[near(contract_state)]
pub struct Contract {
    refuse: bool,
}

#[near(serializers = [json])]
pub struct SignRequestArgs {
    pub payload_v2: Payload,
    pub path: String,
    pub domain_id: u64,
}

#[near(serializers = [json])]
pub enum Payload {
    Ecdsa(String),
    Eddsa(String),
}

#[near(serializers = [json])]
#[serde(tag = "scheme")]
pub enum SignatureResponse {
    Ed25519 { signature: Vec<u8> },
}

fn signing_key(predecessor: &AccountId, path: &str, domain_id: u64) -> SigningKey {
    assert!(
        domain_id == ED25519_DOMAIN_ID,
        "only the ed25519 domain is mocked"
    );
    let seed = Sha256::new()
        .chain_update(predecessor.as_bytes())
        .chain_update(b",")
        .chain_update(path.as_bytes())
        .chain_update(domain_id.to_le_bytes())
        .finalize();
    SigningKey::from_bytes(&seed.into())
}

#[near]
impl Contract {
    #[init]
    pub fn new() -> Self {
        Self { refuse: false }
    }

    pub fn set_refuse(&mut self, refuse: bool) {
        self.refuse = refuse;
    }

    pub fn derived_public_key(
        &self,
        path: String,
        predecessor: Option<AccountId>,
        domain_id: Option<u64>,
    ) -> PublicKey {
        let predecessor = predecessor.unwrap_or_else(env::predecessor_account_id);
        let key = signing_key(&predecessor, &path, domain_id.unwrap_or(0));
        PublicKey::from_parts(CurveType::ED25519, key.verifying_key().to_bytes().to_vec())
            .unwrap_or_else(|_| env::panic_str("an ed25519 key is 32 bytes"))
    }

    #[payable]
    pub fn sign(&mut self, request: SignRequestArgs) -> SignatureResponse {
        assert!(
            env::attached_deposit() >= NearToken::from_yoctonear(1),
            "sign needs a deposit"
        );
        assert!(!self.refuse, "the MPC network did not answer");
        let Payload::Eddsa(payload) = request.payload_v2 else {
            env::panic_str("only Eddsa payloads are mocked")
        };
        let payload = hex::decode(payload).unwrap_or_else(|_| env::panic_str("payload is not hex"));
        let key = signing_key(
            &env::predecessor_account_id(),
            &request.path,
            request.domain_id,
        );
        SignatureResponse::Ed25519 {
            signature: key.sign(&payload).to_bytes().to_vec(),
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod custom_getrandom {
    #![allow(clippy::no_mangle_with_rust_abi)]

    use getrandom::{register_custom_getrandom, Error};
    use near_sdk::env;

    register_custom_getrandom!(custom_getrandom);

    #[allow(clippy::unnecessary_wraps)]
    pub fn custom_getrandom(buf: &mut [u8]) -> Result<(), Error> {
        buf.copy_from_slice(&env::random_seed_array());
        Ok(())
    }
}
