use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_gateway_macros::MethodSpec;
use templar_gateway_types::primitive::{PublicKey, Signature};

/// A single signed intent payload, as accepted by the `execute_intents` method
/// of a NEAR Intents contract. The signature is produced off-chain over the
/// borsh-encoded [`IntentPayload`]; this type only models the JSON wire shape
/// the contract expects.
///
/// The envelope fields (`signature`, `public_key`) are strongly typed and
/// validated at this boundary; they serialize to the exact `<curve>:<base58>`
/// strings the contract reads, so the pre-signed blob is forwarded unchanged.
///
/// `standard` names the off-chain signing standard used to produce `signature`
/// — e.g. `nep413` (the ecosystem-wide NEAR wallet message-signing standard),
/// `erc191` for EVM wallets, `raw_ed25519`, etc. It stays an open `String`
/// because the set the contract accepts is broader than any single value and is
/// validated on-chain; the gateway only forwards it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SignedIntentPayload {
    pub payload: IntentPayload,
    pub standard: String,
    pub signature: Signature,
    pub public_key: PublicKey,
}

/// The NEP-413 message payload wrapped inside a [`SignedIntentPayload`].
///
/// `message` and `nonce` are intentionally left as opaque strings: the NEP-413
/// signature commits to these exact bytes (`message` is the signed JSON string,
/// `nonce` the base58/64 of the signed 32-byte nonce), so the gateway must
/// forward them verbatim — re-structuring them risks a re-serialization that no
/// longer matches what was signed, breaking on-chain verification. Parsing their
/// contents is therefore left to the contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct IntentPayload {
    pub message: String,
    pub nonce: String,
    pub recipient: AccountId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_url: Option<String>,
}

/// Submit one or more pre-signed NEP-413 intents to a NEAR Intents contract
/// (e.g. `intents.near`). The intents are signed off-chain; this op only
/// forwards them via `execute_intents`.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "intents.executeIntents")]
pub struct ExecuteIntents {
    /// The Intents contract to call (`intents.near` on mainnet).
    pub contract_id: AccountId,
    pub signed: Vec<SignedIntentPayload>,
}
