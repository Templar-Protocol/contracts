//! Stand-in for the NEAR MPC signer (`v1.signer`): derived keys and signatures
//! are configured by the test instead of computed, and `sign` returns the
//! configured response in its own receipt like the real contract does.

// `#[near]` method parameters are taken by value; the generated wrappers own them.
#![allow(clippy::needless_pass_by_value)]

use near_sdk::{env, near, serde_json, store::LookupMap, AccountId, NearToken, PanicOnDefault};

#[derive(PanicOnDefault)]
#[near(contract_state)]
pub struct Contract {
    /// `(predecessor, path, domain_id)` → public key text.
    derived_keys: LookupMap<String, String>,
    /// hex payload → the JSON `SignatureResponse` `sign` returns for it.
    signatures: LookupMap<String, String>,
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
pub struct SignArgs {
    pub request: SignRequestArgs,
}

fn derivation_key(predecessor: &AccountId, path: &str, domain_id: u64) -> String {
    format!("{predecessor}|{path}|{domain_id}")
}

#[near]
impl Contract {
    #[init]
    pub fn new() -> Self {
        Self {
            derived_keys: LookupMap::new(b"k"),
            signatures: LookupMap::new(b"s"),
        }
    }

    pub fn set_derived_public_key(
        &mut self,
        predecessor: AccountId,
        path: String,
        domain_id: u64,
        public_key: String,
    ) {
        self.derived_keys
            .insert(derivation_key(&predecessor, &path, domain_id), public_key);
    }

    /// `response` is the JSON the real contract would return, e.g.
    /// `{"scheme":"Ed25519","signature":[…]}`.
    pub fn set_signature(&mut self, payload_hex: String, response: serde_json::Value) {
        self.signatures.insert(payload_hex, response.to_string());
    }

    pub fn derived_public_key(
        &self,
        path: String,
        predecessor: Option<AccountId>,
        domain_id: Option<u64>,
    ) -> String {
        let predecessor = predecessor.unwrap_or_else(env::predecessor_account_id);
        let key = derivation_key(&predecessor, &path, domain_id.unwrap_or(0));
        self.derived_keys
            .get(&key)
            .unwrap_or_else(|| env::panic_str(&format!("no derived key configured for {key}")))
            .clone()
    }

    #[payable]
    pub fn sign(&mut self, request: SignRequestArgs) -> serde_json::Value {
        assert!(
            env::attached_deposit() >= NearToken::from_yoctonear(1),
            "sign needs a deposit"
        );
        let payload_hex = match request.payload_v2 {
            Payload::Ecdsa(hex) | Payload::Eddsa(hex) => hex,
        };
        let response = self.signatures.get(&payload_hex).unwrap_or_else(|| {
            env::panic_str(&format!("no signature configured for {payload_hex}"))
        });
        serde_json::from_str(response)
            .unwrap_or_else(|_| env::panic_str("stored response is not JSON"))
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
