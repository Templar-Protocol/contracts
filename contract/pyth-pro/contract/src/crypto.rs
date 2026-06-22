use near_sdk::env;
use templar_pyth_pro_verifier::Crypto;

/// [`Crypto`] backed by NEAR's native `env::ed25519_verify` (ed25519 is NEAR's own scheme).
pub struct EnvCrypto;

impl Crypto for EnvCrypto {
    fn ed25519_verify(&self, signature: &[u8; 64], message: &[u8], public_key: &[u8; 32]) -> bool {
        env::ed25519_verify(signature, message, public_key)
    }
}
