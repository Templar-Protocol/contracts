/// Cryptographic primitive the verifier needs, supplied by the host runtime.
///
/// On NEAR this maps directly onto `env::ed25519_verify` (ed25519 is NEAR's native signature
/// scheme); keeping it behind a trait lets the verification logic stay chain-agnostic and
/// unit-testable off-chain.
pub trait Crypto {
    /// Verify that `signature` (64 bytes) over `message` is valid for the ed25519 `public_key`.
    fn ed25519_verify(&self, signature: &[u8; 64], message: &[u8], public_key: &[u8; 32]) -> bool;
}
