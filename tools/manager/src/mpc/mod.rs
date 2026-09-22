//! DAO-governed MPC signing: a Sputnik proposal asks the NEAR MPC signer to sign
//! the hash of a payload whose bytes travel in the proposal or a local file, and
//! the signature is read back from the executing receipt chain.

pub mod dao;
pub mod envelope;
pub mod payload;
pub mod signer_contract;
