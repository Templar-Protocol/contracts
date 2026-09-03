//! Stellar-side checked native adapter: secret custody and authorization
//! boundaries. All operations are pure decisions over typed inputs; no live
//! chain mutation happens here.

use std::collections::BTreeSet;
use std::env;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use soroban_client::keypair::{Keypair, KeypairBehavior};
use stellar_strkey::ed25519::PrivateKey;
use zeroize::Zeroizing;

use crate::domain::{Environment, OperationV1};
use crate::error::{Error, Result};

/// Encoded length of a Stellar `S...` secret seed strkey.
pub const SEED_STRKEY_LEN: usize = 56;

/// Named-environment-variable secret provider. The seed is accepted only
/// through the named env var, validated and decoded as an Ed25519 `S...`
/// strkey, held zeroized, and used to derive the `G...` public key.
#[derive(Debug)]
pub struct StellarSecretProviderV1 {
    env_var: String,
    seed: Zeroizing<Vec<u8>>,
    public_key: String,
}

impl StellarSecretProviderV1 {
    /// Reads the named environment variable, checks and decodes the `S...`
    /// seed, and derives the public key. The raw variable and the decoded
    /// seed bytes are zeroized when dropped.
    pub fn from_named_env(env_var: &str) -> Result<Self> {
        if env_var.trim().is_empty() {
            return Err(Error::InvalidInput(
                "secret env var name must not be empty".into(),
            ));
        }
        let raw = Zeroizing::new(env::var(env_var).map_err(|_| {
            Error::InvalidInput(format!("environment variable {env_var} is not set"))
        })?);
        let trimmed = raw.trim();
        if trimmed.len() != SEED_STRKEY_LEN || !trimmed.starts_with('S') {
            return Err(Error::InvalidInput(format!(
                "environment variable {env_var} must hold a {SEED_STRKEY_LEN}-character S... seed strkey"
            )));
        }
        let decoded = PrivateKey::from_str(trimmed)
            .map_err(|error| Error::InvalidInput(format!("invalid S... seed strkey: {error}")))?;
        let keypair = Keypair::from_secret(trimmed)
            .map_err(|error| Error::InvalidInput(format!("seed rejected by keypair: {error}")))?;
        Ok(Self {
            env_var: env_var.to_string(),
            seed: Zeroizing::new(decoded.0.to_vec()),
            public_key: keypair.public_key(),
        })
    }

    /// Name of the environment variable the seed was read from.
    pub fn env_var(&self) -> &str {
        &self.env_var
    }

    /// Derived `G...` public key strkey.
    pub fn public_key(&self) -> &str {
        &self.public_key
    }

    /// Raw decoded 32-byte seed. Zeroized when the provider drops.
    pub fn seed(&self) -> &[u8] {
        &self.seed
    }
}

/// Authorization classes the v1 Stellar adapter distinguishes. Address and
/// contract auth is not a class: it is observed evidence and is refused in v1.
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StellarAuthorizationClass {
    /// Explicit OFT role operator entry.
    OftRoleOperator,
    /// Owner-derived AUTHORIZER.
    OwnerDerivedAuthorizer,
    /// OApp/Endpoint delegate.
    OAppDelegate,
}

/// Observed signer evidence for one operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StellarSignerEvidenceV1 {
    /// Signer holds an explicit OFT role operator entry.
    OftRoleOperator { role: String, address: String },
    /// Signer is the owner-derived AUTHORIZER.
    OwnerDerivedAuthorizer { owner: String },
    /// Signer is the OApp/Endpoint delegate.
    OAppDelegate { delegate: String },
    /// Signer authorized through an Address/contract entry; refused in v1.
    AddressContract { address: String },
}

impl StellarSignerEvidenceV1 {
    /// Authorization class of the evidence, if not contract auth.
    pub fn class(&self) -> Option<StellarAuthorizationClass> {
        match self {
            Self::OftRoleOperator { .. } => Some(StellarAuthorizationClass::OftRoleOperator),
            Self::OwnerDerivedAuthorizer { .. } => {
                Some(StellarAuthorizationClass::OwnerDerivedAuthorizer)
            }
            Self::OAppDelegate { .. } => Some(StellarAuthorizationClass::OAppDelegate),
            Self::AddressContract { .. } => None,
        }
    }

    /// Address of the signer implied by the evidence.
    pub fn signer(&self) -> &str {
        match self {
            Self::OftRoleOperator { address, .. }
            | Self::OwnerDerivedAuthorizer { owner: address }
            | Self::OAppDelegate { delegate: address }
            | Self::AddressContract { address } => address,
        }
    }
}

/// Per-operation authorization spec.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StellarAuthorizationSpecV1 {
    pub operation: String,
    pub allowed_classes: BTreeSet<StellarAuthorizationClass>,
    pub require_source_account: bool,
}

/// Typed authorization request boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuthorizationRequestV1 {
    pub environment: Environment,
    pub operation: OperationV1,
    pub source_account: String,
    pub evidence: StellarSignerEvidenceV1,
}

/// Typed authorization decision boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuthorizationDecisionV1 {
    pub operation: String,
    pub signer: String,
    pub class: StellarAuthorizationClass,
    pub source_account_auth: bool,
}

/// Checked native adapter trait for Stellar authorization decisions.
pub trait StellarAuthorizationAdapter {
    fn authorize(&self, request: &AuthorizationRequestV1) -> Result<AuthorizationDecisionV1>;
}

/// Authorization spec for an operation. Config-mutating Stellar operations
/// accept operator and owner-derived AUTHORIZER; message send additionally
/// accepts the OApp/Endpoint delegate. Every op requires source-account auth.
pub fn authorization_spec_v1(operation: &OperationV1) -> StellarAuthorizationSpecV1 {
    let mut allowed_classes = BTreeSet::new();
    allowed_classes.insert(StellarAuthorizationClass::OftRoleOperator);
    allowed_classes.insert(StellarAuthorizationClass::OwnerDerivedAuthorizer);
    if matches!(operation, OperationV1::SendLeg { .. }) {
        allowed_classes.insert(StellarAuthorizationClass::OAppDelegate);
    }
    StellarAuthorizationSpecV1 {
        operation: operation_label(operation),
        allowed_classes,
        require_source_account: true,
    }
}

/// Stable operation label used in decisions and refusals.
pub fn operation_label(operation: &OperationV1) -> String {
    match operation {
        OperationV1::InstallStellarWasm { .. } => "install_stellar_wasm",
        OperationV1::DeployStellarOft { .. } => "deploy_stellar_oft",
        OperationV1::DeployEvmOft { .. } => "deploy_evm_oft",
        OperationV1::BeginStellarOwnershipTransfer { .. } => "begin_stellar_ownership_transfer",
        OperationV1::AcceptStellarOwnership => "accept_stellar_ownership",
        OperationV1::CancelStellarOwnershipTransfer => "cancel_stellar_ownership_transfer",
        OperationV1::TransferEvmOwnership { .. } => "transfer_evm_ownership",
        OperationV1::SetStellarDelegate { .. } => "set_stellar_delegate",
        OperationV1::SetEvmDelegate { .. } => "set_evm_delegate",
        OperationV1::SetStellarPeer { .. } => "set_peer_stellar",
        OperationV1::SetEvmPeer { .. } => "set_peer_evm",
        OperationV1::SetStellarSendLibrary { .. } => "set_send_library_stellar",
        OperationV1::SetStellarReceiveLibrary { .. } => "set_receive_library_stellar",
        OperationV1::RemoveStellarReceiveLibraryTimeout { .. } => {
            "remove_receive_library_timeout_stellar"
        }
        OperationV1::SetEvmSendLibrary { .. } => "set_send_library_evm",
        OperationV1::SetEvmReceiveLibrary { .. } => "set_receive_library_evm",
        OperationV1::RemoveEvmReceiveLibraryTimeout { .. } => "remove_receive_library_timeout_evm",
        OperationV1::SetStellarUlnConfig { .. } => "set_uln_stellar",
        OperationV1::SetEvmUlnConfig { .. } => "set_uln_evm",
        OperationV1::SetStellarExecutorConfig { .. } => "set_executor_stellar",
        OperationV1::SetEvmExecutorConfig { .. } => "set_executor_evm",
        OperationV1::SetStellarReceiveOptions { .. } => "set_options_stellar",
        OperationV1::SetEvmReceiveOptions { .. } => "set_options_evm",
        OperationV1::SetDefaultFee { .. } => "set_default_fee",
        OperationV1::SetDestinationFee { .. } => "set_destination_fee",
        OperationV1::SetFeeRecipient { .. } => "set_fee_recipient",
        OperationV1::SetMessageInspector { .. } => "set_message_inspector",
        OperationV1::SetInboundRateLimit { .. } => "set_inbound_rate_limit",
        OperationV1::SetOutboundRateLimit { .. } => "set_outbound_rate_limit",
        OperationV1::PauseEmergency => "pause_emergency",
        OperationV1::UnpauseEmergency => "unpause_emergency",
        OperationV1::SetTtlConfig { .. } => "set_ttl_config",
        OperationV1::FreezeTtlConfig { .. } => "freeze_ttl_config",
        OperationV1::ExtendInstanceTtl { .. } => "extend_instance_ttl",
        OperationV1::GrantRole { .. } => "grant_role",
        OperationV1::RevokeRole { .. } => "revoke_role",
        OperationV1::SetRoleAdmin { .. } => "set_role_admin",
        OperationV1::RemoveRoleAdmin { .. } => "remove_role_admin",
        OperationV1::SendLeg { .. } => "send_leg",
        OperationV1::CommitVerification { .. } => "commit_verification",
        OperationV1::ExecuteReceive { .. } => "execute_receive",
        OperationV1::ContainOutbound { .. } => "contain_outbound",
        OperationV1::RestoreOutbound { .. } => "restore_outbound",
        OperationV1::RestoreFootprint { .. } => "restore_footprint",
    }
    .to_string()
}

/// Checked authorization decision. Returns the decision or a policy refusal
/// naming the failing boundary: contract auth, unsupported class, or missing
/// source-account auth.
pub fn authorize(request: &AuthorizationRequestV1) -> Result<AuthorizationDecisionV1> {
    let spec = authorization_spec_v1(&request.operation);
    let (class, signer) = match &request.evidence {
        StellarSignerEvidenceV1::AddressContract { address } => {
            return Err(Error::Policy(format!(
                "address_contract_auth_unsupported_v1: {address}"
            )));
        }
        StellarSignerEvidenceV1::OftRoleOperator { address, .. } => {
            (StellarAuthorizationClass::OftRoleOperator, address)
        }
        StellarSignerEvidenceV1::OwnerDerivedAuthorizer { owner } => {
            (StellarAuthorizationClass::OwnerDerivedAuthorizer, owner)
        }
        StellarSignerEvidenceV1::OAppDelegate { delegate } => {
            (StellarAuthorizationClass::OAppDelegate, delegate)
        }
    };
    if !spec.allowed_classes.contains(&class) {
        return Err(Error::Policy(format!(
            "stellar_authorization_class_unsupported_v1: {:?} on {}",
            class, spec.operation
        )));
    }
    let signer: &str = signer.as_str();
    if spec.require_source_account && signer != request.source_account {
        return Err(Error::Policy(format!(
            "source_account_auth_required_v1: signer {signer} is not source account {}",
            request.source_account
        )));
    }
    Ok(AuthorizationDecisionV1 {
        operation: spec.operation,
        signer: signer.to_string(),
        class,
        source_account_auth: true,
    })
}

/// Checked adapter implementation of [`StellarAuthorizationAdapter`].
#[derive(Debug, Default)]
pub struct CheckedStellarAuthorization;

impl StellarAuthorizationAdapter for CheckedStellarAuthorization {
    fn authorize(&self, request: &AuthorizationRequestV1) -> Result<AuthorizationDecisionV1> {
        authorize(request)
    }
}

/// Observed on-chain role state for one route.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StellarRoleStateV1 {
    pub owner: String,
    pub operator: Option<String>,
    pub delegate: Option<String>,
}

/// One role or delegate drift between desired and observed state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StellarRoleDriftV1 {
    pub field: &'static str,
    pub desired: String,
    pub observed: String,
}

/// Compares desired owner/operator/delegate against observed role state.
/// The explicit OFT role operator entry must exist and match the desired
/// delegate; missing entries are drift.
pub fn role_drift(
    desired_owner: &str,
    desired_delegate: &str,
    observed: &StellarRoleStateV1,
) -> Vec<StellarRoleDriftV1> {
    let mut drift = Vec::new();
    if observed.owner != desired_owner {
        drift.push(StellarRoleDriftV1 {
            field: "owner",
            desired: desired_owner.to_string(),
            observed: observed.owner.clone(),
        });
    }
    let operator = observed
        .operator
        .clone()
        .unwrap_or_else(|| "missing".to_string());
    if operator != desired_delegate {
        drift.push(StellarRoleDriftV1 {
            field: "operator",
            desired: desired_delegate.to_string(),
            observed: operator,
        });
    }
    let delegate = observed
        .delegate
        .clone()
        .unwrap_or_else(|| "missing".to_string());
    if delegate != desired_delegate {
        drift.push(StellarRoleDriftV1 {
            field: "delegate",
            desired: desired_delegate.to_string(),
            observed: delegate,
        });
    }

    drift
}
/// Typed marker a qualified simulation returns when the source account's
/// Soroban footprint needs restoration before the original transaction can
/// proceed.
pub const RESTORATION_REQUIRED: &str = "restoration_required";

/// RPC-backed simulation outcome for the exact typed transaction a
/// proposal commits to. Every field is adapter-derived; no value is ever
/// fabricated or defaulted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StellarSimulationV1 {
    /// Base64 transaction envelope XDR of the assembled, broadcast-exact
    /// transaction.
    pub envelope_xdr: String,
    /// Hex network-bound hash of the assembled envelope.
    pub envelope_sha256: String,
    /// Base64 Soroban authorization entries; the adapter guarantees only
    /// source-account credential entries in v1.
    pub auth_entries: Vec<String>,
    /// Ledger sequence the simulation ran against.
    pub simulation_ledger: u32,
}

/// Observable Stellar chain boundary for proposal construction. Live reads
/// beyond passphrase/ledger/sequence remain pending the native-mutation
/// qualification gate and fail closed with a typed refusal — never a
/// fabricated value.
pub trait StellarChain {
    /// RPC-reported network passphrase.
    fn network_passphrase(&self) -> Result<String>;
    /// Live EndpointV2 `eid()` view for the configured endpoint.
    fn endpoint_eid(&self, endpoint: &str) -> Result<u32>;
    /// Current account sequence as a decimal string.
    fn account_sequence(&self, account: &str) -> Result<String>;
    /// Live signer weights for `account`, keyed by public key.
    fn account_signers(&self, account: &str) -> Result<std::collections::BTreeMap<String, u32>>;
    /// Required threshold weight for `level` (`low`/`medium`/`high`).
    fn account_threshold(&self, account: &str, level: &str) -> Result<u32>;
    /// Sequence number of the latest ledger known to the RPC.
    fn latest_ledger(&self) -> Result<u32>;
    /// Constructs, simulates, and assembles the exact typed Soroban
    /// transaction a proposal commits to, from the live source account,
    /// sequence, and ledger bounds. The returned envelope is the exact
    /// object that would be broadcast; simulation failures, footprint
    /// restoration, and non-source-account auth classes are typed
    /// refusals, never silent markers.
    fn simulate_transaction(
        &self,
        operation: &OperationV1,
        source: &str,
        sequence: &str,
        min_ledger: u32,
        max_ledger: u32,
    ) -> Result<StellarSimulationV1>;
}

/// Live Soroban RPC implementation of [`StellarChain`] over
/// `soroban-client 0.5.5`. Read methods with direct RPC support are
/// implemented; account-entry decoding (signers/thresholds) and endpoint
/// views wait on the pinned qualification gate.
pub struct HttpStellarChain {
    server: soroban_client::Server,
}

impl HttpStellarChain {
    pub fn new(url: &str) -> Result<Self> {
        let parsed = reqwest::Url::parse(url)
            .map_err(|error| Error::InvalidInput(format!("invalid Stellar RPC URL: {error}")))?;
        if parsed.scheme() != "https" {
            return Err(Error::InvalidInput(
                "Stellar RPC URL must be https in v1".into(),
            ));
        }
        let server = soroban_client::Server::new(url, soroban_client::Options::default())
            .map_err(|error| Error::Chain(format!("stellar rpc connect failed: {error}")))?;
        Ok(Self { server })
    }
}

fn rpc<T>(result: std::result::Result<T, soroban_client::error::Error>) -> Result<T> {
    result.map_err(|error| Error::Chain(format!("stellar rpc call failed: {error:?}")))
}

fn pending_qualification(what: &str) -> Error {
    Error::Chain(format!(
        "stellar live read '{what}' is disabled pending the native-mutation qualification gate"
    ))
}

impl StellarChain for HttpStellarChain {
    fn network_passphrase(&self) -> Result<String> {
        let response = rpc(crate::block_on(self.server.get_network())?)?;
        response
            .passphrase
            .ok_or_else(|| Error::Chain("stellar rpc omitted network passphrase".into()))
    }

    fn endpoint_eid(&self, _endpoint: &str) -> Result<u32> {
        Err(pending_qualification("endpoint_eid"))
    }

    fn account_sequence(&self, account: &str) -> Result<String> {
        use stellar_baselib::account::AccountBehavior as _;
        let loaded = rpc(crate::block_on(self.server.get_account(account))?)?;
        Ok(loaded.sequence_number())
    }

    fn account_signers(&self, _account: &str) -> Result<std::collections::BTreeMap<String, u32>> {
        Err(pending_qualification("account_signers"))
    }

    fn account_threshold(&self, _account: &str, _level: &str) -> Result<u32> {
        Err(pending_qualification("account_threshold"))
    }

    fn latest_ledger(&self) -> Result<u32> {
        let response = rpc(crate::block_on(self.server.get_latest_ledger())?)?;
        Ok(response.sequence)
    }

    fn simulate_transaction(
        &self,
        _operation: &OperationV1,
        _source: &str,
        _sequence: &str,
        _min_ledger: u32,
        _max_ledger: u32,
    ) -> Result<StellarSimulationV1> {
        Err(pending_qualification("simulate_transaction"))
    }
}
