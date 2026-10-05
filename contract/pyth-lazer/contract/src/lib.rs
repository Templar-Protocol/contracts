#![allow(clippy::needless_pass_by_value)]
//! Pyth Lazer oracle adapter for NEAR.
//!
//! A push-style adapter: anyone may relay a Pyth Lazer signed price payload via
//! [`Contract::update_price_feeds`]; the adapter verifies it (ed25519 signature against a
//! trusted, non-expired signer set, channel filter, freshness window, and a monotonic-per-feed
//! timestamp that blocks replays) and stores the prices. Consumers read those prices by the
//! native Lazer `u32` feed id — the adapter is consumed by wrapping it in a proxy-oracle as a
//! `Lazer` source, not as a direct `pyth-oracle.near` drop-in.
//!
//! Governance is intentionally minimal: a single owner (via [`near_sdk_contract_tools::Owner`])
//! gates the `admin_*` methods.

mod crypto;
mod events;
mod state;

use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};

use near_sdk::{
    assert_one_yocto, env,
    json_types::{Base64VecU8, I64},
    near, AccountId, Gas, NearToken, PanicOnDefault, Promise,
};
use near_sdk_contract_tools::{owner::Owner, utils::apply_storage_fee_and_refund, Owner};
use templar_common::{
    oracle::lazer::{
        feed_data_from_parsed, FeedData, FeedDataResponse, ParsedFeedView, VerifiedUpdateView,
    },
    upgrade::{UpgradeSource, MIGRATE_METHOD},
    versioned_state::{impl_versioned_state, StateVersion, VersionedState},
    Nanoseconds, UnwrapReject,
};
use templar_pyth_lazer_verifier as verifier;

use crate::crypto::EnvCrypto;
use crate::events::PythLazerEvent;
use crate::state::State;

/// A trusted Pyth Lazer publisher: its 32-byte ed25519 public key (hex-encoded in JSON) and the
/// unix-seconds instant after which its signatures are no longer accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
pub struct TrustedSigner {
    #[serde(
        serialize_with = "hex::serde::serialize",
        deserialize_with = "hex::serde::deserialize"
    )]
    pub public_key: [u8; 32],
    pub expires_at_s: u64,
}

/// A validated set of trusted signers: non-empty, free of duplicate public keys, and bounded in
/// size. Backed by a `BTreeMap<public_key, expires_at_s>`, so the "one expiry per key" uniqueness
/// invariant is structural (enforced by the map) rather than re-checked on every mutation, and
/// iteration order is deterministic (sorted by public key). Serializes transparently as the inner
/// array of [`TrustedSigner`] for both Borsh and JSON, so the stored layout and `get_config` shape
/// are unchanged (the array is just emitted in public-key order).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignerSet(BTreeMap<[u8; 32], u64>);

impl SignerSet {
    /// Maximum number of trusted signers (Pyth Lazer uses a small publisher set).
    pub const MAX: usize = 32;

    /// Validate a list of signers into the set.
    fn try_new(signers: Vec<TrustedSigner>) -> Result<Self, &'static str> {
        if signers.is_empty() {
            return Err("config: signer set must not be empty");
        }
        if signers.len() > Self::MAX {
            return Err("config: too many signers");
        }
        let mut map = BTreeMap::new();
        for signer in signers {
            // A duplicate key collides in the map; that's the uniqueness invariant made structural.
            if map.insert(signer.public_key, signer.expires_at_s).is_some() {
                return Err("config: duplicate signer public key");
            }
        }
        Ok(Self(map))
    }

    /// Add a new signer, or refresh the expiry of an existing one (matched by public key).
    fn upsert(&mut self, signer: TrustedSigner) -> Result<(), &'static str> {
        if !self.0.contains_key(&signer.public_key) && self.0.len() >= Self::MAX {
            return Err("config: too many signers");
        }
        self.0.insert(signer.public_key, signer.expires_at_s);
        Ok(())
    }

    /// Remove the signer with `public_key`. Rejects removing the final signer (which would brick
    /// verification); removing an absent key is a no-op.
    fn remove(&mut self, public_key: &[u8; 32]) -> Result<(), &'static str> {
        if self.0.len() == 1 && self.0.contains_key(public_key) {
            return Err("config: cannot remove the last signer");
        }
        self.0.remove(public_key);
        Ok(())
    }

    /// The trusted signers in the verifier's chain-agnostic representation.
    fn verifier_signers(&self) -> Vec<verifier::TrustedSigner> {
        self.0
            .iter()
            .map(|(&public_key, &expires_at_s)| verifier::TrustedSigner {
                public_key,
                expires_at_s,
            })
            .collect()
    }

    /// The signers as the public [`TrustedSigner`] DTO (the serialized form), in public-key order.
    fn to_vec(&self) -> Vec<TrustedSigner> {
        self.0
            .iter()
            .map(|(&public_key, &expires_at_s)| TrustedSigner {
                public_key,
                expires_at_s,
            })
            .collect()
    }
}

// Serialize transparently as `Vec<TrustedSigner>` (in public-key order) and validate back through
// `try_new` on the way in — the map is an internal representation detail, not a wire format.
impl near_sdk::borsh::BorshSerialize for SignerSet {
    fn serialize<W: near_sdk::borsh::io::Write>(
        &self,
        writer: &mut W,
    ) -> near_sdk::borsh::io::Result<()> {
        near_sdk::borsh::BorshSerialize::serialize(&self.to_vec(), writer)
    }
}

impl near_sdk::borsh::BorshDeserialize for SignerSet {
    fn deserialize_reader<R: near_sdk::borsh::io::Read>(
        reader: &mut R,
    ) -> near_sdk::borsh::io::Result<Self> {
        let signers = Vec::<TrustedSigner>::deserialize_reader(reader)?;
        Self::try_new(signers).map_err(|e| {
            near_sdk::borsh::io::Error::new(near_sdk::borsh::io::ErrorKind::InvalidData, e)
        })
    }
}

impl near_sdk::serde::Serialize for SignerSet {
    fn serialize<S: near_sdk::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        near_sdk::serde::Serialize::serialize(&self.to_vec(), serializer)
    }
}

impl<'de> near_sdk::serde::Deserialize<'de> for SignerSet {
    fn deserialize<D: near_sdk::serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        let signers = Vec::<TrustedSigner>::deserialize(deserializer)?;
        Self::try_new(signers).map_err(near_sdk::serde::de::Error::custom)
    }
}

// ABI schemas. `SignerSet` has hand-written Borsh/serde impls (transparent over `Vec<TrustedSigner>`,
// validated on the way in), so the `#[near]` macro can't derive its schemas — and the `#[near]`
// schema on `Config` recurses into this field under near-sdk's `abi` build. These impls are
// unconditional (matching `templar-common`'s `Wad`/`Number`): they describe the wire form via
// always-available primitives, so they need neither an `abi` feature gate nor `TrustedSigner`'s own
// (abi-only) schemas.
impl near_sdk::borsh::BorshSchema for SignerSet {
    fn add_definitions_recursively(
        definitions: &mut std::collections::BTreeMap<
            near_sdk::borsh::schema::Declaration,
            near_sdk::borsh::schema::Definition,
        >,
    ) {
        // A `TrustedSigner` (`[u8; 32]` then `u64`) is Borsh-identical to that tuple, so the set is
        // byte-for-byte a `Vec<([u8; 32], u64)>`.
        <Vec<([u8; 32], u64)> as near_sdk::borsh::BorshSchema>::add_definitions_recursively(
            definitions,
        );
    }

    fn declaration() -> near_sdk::borsh::schema::Declaration {
        <Vec<([u8; 32], u64)> as near_sdk::borsh::BorshSchema>::declaration()
    }
}

impl schemars::JsonSchema for SignerSet {
    fn schema_name() -> String {
        "SignerSet".to_string()
    }

    fn json_schema(generator: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        // The JSON form is an array of `TrustedSigner` objects (`public_key` as a hex string). Mirror
        // that with a schema-only DTO so this impl never needs `TrustedSigner: JsonSchema`.
        #[derive(schemars::JsonSchema)]
        #[allow(dead_code)]
        struct TrustedSignerSchema {
            #[schemars(with = "String")]
            public_key: [u8; 32],
            expires_at_s: u64,
        }
        <Vec<TrustedSignerSchema> as schemars::JsonSchema>::json_schema(generator)
    }

    fn is_referenceable() -> bool {
        false
    }
}

/// Caller-supplied verification policy, validated into a [`Config`] by `new` / `admin_set_config`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json])]
pub struct ConfigArgs {
    pub signers: Vec<TrustedSigner>,
    pub max_timestamp_delay_s: u64,
    pub max_timestamp_ahead_s: u64,
    pub allowed_channel_id: Option<u8>,
    pub update_fee: NearToken,
    pub max_feeds_per_update: u32,
}

/// Validated verification policy for incoming payloads. Built only via `Config::try_from` a
/// [`ConfigArgs`], so the invariants live here rather than in the callers.
#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
pub struct Config {
    /// Accepted publisher signers (non-empty, deduplicated, bounded).
    pub signers: SignerSet,
    /// Reject payloads whose timestamp is older than this many seconds (must be non-zero).
    pub max_timestamp_delay_s: u64,
    /// Reject payloads more than this many seconds in the future (`0` = strict, no future).
    pub max_timestamp_ahead_s: u64,
    /// If set, only accept payloads on this Lazer channel id (e.g. 1 = real-time).
    pub allowed_channel_id: Option<u8>,
    /// Fee charged per `update_price_feeds` call, on top of the storage cost, and retained by the
    /// contract. Defaults to zero (no fee).
    pub update_fee: NearToken,
    /// Upper bound (must be non-zero) on how many feeds a single `update_price_feeds` call may
    /// store. The emitted NEP-297 `UpdatePrices` event carries the full `FeedData` for every
    /// updated feed, so an oversized signed bundle could otherwise push the receipt's total log
    /// length past NEAR's limit and abort the call *after* verification + storage writes. Capping
    /// the accepted feed count keeps the event (and the per-call work) bounded. Bundle size is set
    /// by a trusted signer, not the caller, so this is a robustness guard rather than an anti-abuse
    /// control.
    pub max_feeds_per_update: u32,
}

impl TryFrom<ConfigArgs> for Config {
    type Error = &'static str;

    fn try_from(args: ConfigArgs) -> Result<Self, Self::Error> {
        if args.max_timestamp_delay_s == 0 {
            return Err("config: max_timestamp_delay_s must be non-zero");
        }
        if args.max_feeds_per_update == 0 {
            return Err("config: max_feeds_per_update must be non-zero");
        }
        Ok(Self {
            signers: SignerSet::try_new(args.signers)?,
            max_timestamp_delay_s: args.max_timestamp_delay_s,
            max_timestamp_ahead_s: args.max_timestamp_ahead_s,
            allowed_channel_id: args.allowed_channel_id,
            update_fee: args.update_fee,
            max_feeds_per_update: args.max_feeds_per_update,
        })
    }
}

fn parsed_feed_view(feed: &verifier::ParsedFeed) -> ParsedFeedView {
    ParsedFeedView {
        feed_id: feed.feed_id,
        price: feed.price.map(I64),
        best_bid_price: feed.best_bid_price.map(I64),
        best_ask_price: feed.best_ask_price.map(I64),
        publisher_count: feed.publisher_count,
        exponent: feed.exponent,
        confidence: feed.confidence.map(I64),
        funding_rate: feed.funding_rate.map(I64),
        funding_timestamp_ns: feed.funding_timestamp,
        funding_rate_interval_ns: feed.funding_rate_interval,
        market_session: feed.market_session,
        ema_price: feed.ema_price.map(I64),
        ema_confidence: feed.ema_confidence.map(I64),
        feed_update_timestamp_ns: feed.feed_update_timestamp,
    }
}

fn verified_update_view(update: verifier::VerifiedUpdate) -> VerifiedUpdateView {
    VerifiedUpdateView {
        signer: update.signer,
        channel_id: update.channel_id,
        timestamp_ns: update.timestamp,
        feeds: update.feeds.iter().map(parsed_feed_view).collect(),
    }
}

#[derive(Owner, PanicOnDefault)]
#[near(contract_state)]
pub struct Contract {
    pub state: VersionedState<State>,
}

// Generates the private `migrate()` entrypoint (driven by `state::migration::Migration`) plus the
// `get_stored_state_version` / `get_target_state_version` / `needs_migration` views.
impl_versioned_state!(Contract, State, crate::state::migration::Migration);

// The live fields (`config`, `feeds`) live on `State`; deref so the rest of the contract can keep
// reaching them as `self.config` / `self.feeds`.
impl Deref for Contract {
    type Target = State;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl DerefMut for Contract {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

#[near]
impl Contract {
    /// Gas reserved for the batched `migrate` call in [`Self::admin_upgrade`].
    pub const GAS_FOR_MIGRATE: Gas = Gas::from_tgas(250);

    #[init]
    pub fn new(owner: AccountId, config: ConfigArgs) -> Self {
        let config = Config::try_from(config).unwrap_or_else(|e| env::panic_str(e));
        let mut contract = Self {
            state: State::new(config),
        };
        Owner::init(&mut contract, &owner);
        contract
    }

    pub fn get_config(&self) -> &Config {
        &self.config
    }

    /// Replace the whole verification policy (validated before it is stored).
    #[payable]
    pub fn admin_set_config(&mut self, config: ConfigArgs) {
        assert_one_yocto();
        Self::require_owner();
        self.config = Config::try_from(config).unwrap_or_else(|e| env::panic_str(e));
        PythLazerEvent::ConfigSet {
            config: self.config.clone(),
        }
        .emit();
    }

    /// Add/refresh (`expires_at_s = Some`) or remove (`expires_at_s = None`) a trusted signer.
    /// `public_key` is a 64-character hex ed25519 key (an optional `0x` prefix is accepted).
    /// Removing the last signer is rejected; `SignerSet` upholds dedup and the size bound.
    #[payable]
    pub fn admin_set_signer(&mut self, public_key: String, expires_at_s: Option<u64>) {
        assert_one_yocto();
        Self::require_owner();

        let mut bytes = [0u8; 32];
        hex::decode_to_slice(public_key.trim_start_matches("0x"), &mut bytes)
            .unwrap_or_else(|_| env::panic_str("invalid signer public key: expected 32-byte hex"));

        let result = match expires_at_s {
            Some(expires_at_s) => self.config.signers.upsert(TrustedSigner {
                public_key: bytes,
                expires_at_s,
            }),
            None => self.config.signers.remove(&bytes),
        };
        result.unwrap_or_else(|e| env::panic_str(e));

        // Emit only after the upsert/remove succeeds, so a rejected op (last-signer removal, size
        // bound) emits nothing. `public_key` is normalized to canonical hex, matching `get_config`.
        match expires_at_s {
            Some(expires_at_s) => PythLazerEvent::SignerUpserted {
                public_key: hex::encode(bytes),
                expires_at_s,
            }
            .emit(),
            None => PythLazerEvent::SignerRemoved {
                public_key: hex::encode(bytes),
            }
            .emit(),
        }
    }

    /// Withdraw `amount` of accrued fees (or any free balance) to the owner. The NEAR runtime's
    /// storage-staking guard rejects any withdrawal that would drop the balance below the
    /// contract's staked storage requirement.
    #[payable]
    pub fn admin_withdraw(&mut self, amount: NearToken) -> Promise {
        assert_one_yocto();
        Self::require_owner();
        // `require_owner` guarantees the predecessor is the owner.
        let receiver = env::predecessor_account_id();
        PythLazerEvent::Withdrawn {
            amount,
            receiver: receiver.clone(),
        }
        .emit();
        Promise::new(receiver).transfer(amount)
    }

    /// Atomically deploy new contract code and run its `migrate` in a single receipt: a failed
    /// migration reverts the code deployment too. `migrate_args` is the JSON-encoded
    /// `state::migration::Migration` selecting the state transform to apply (none exist at v1, so
    /// this is the seam for future upgrades). The batched `migrate` is private — the runtime calls
    /// it as this account, so only the owner-gated path here can trigger it.
    #[payable]
    pub fn admin_upgrade(&mut self, code: UpgradeSource, migrate_args: Base64VecU8) -> Promise {
        assert_one_yocto();
        Self::require_owner();
        // An empty blob can never deploy; reject up front (mirrors the governance path's
        // `EmptyUpgradeCode` guard) so no misleading event is logged and no gas is spent on a
        // doomed deploy.
        if code.is_empty_code() {
            env::panic_str("admin_upgrade: empty code blob");
        }
        // See PythLazerEvent::Upgraded — logs the scheduled upgrade before the deploy+migrate Promise.
        PythLazerEvent::Upgraded {
            code: code.summary(),
            migrated: !migrate_args.0.is_empty(),
        }
        .emit();
        code.deploy_and_migrate(MIGRATE_METHOD, migrate_args, Self::GAS_FOR_MIGRATE)
    }

    /// Verify a Pyth Lazer solana-format (ed25519) signed payload and store its feeds. Permissionless:
    /// authenticity is enforced cryptographically, and the per-feed monotonic timestamp check
    /// prevents replays and out-of-order writes.
    ///
    /// The caller must attach a deposit covering the storage newly consumed by this call plus the
    /// configured [`Config::update_fee`]; any excess is refunded. Updates that only overwrite
    /// existing feeds consume no new storage, so (with a zero fee) they cost nothing.
    #[payable]
    pub fn update_price_feeds(&mut self, payload: Base64VecU8) {
        let storage_before = env::storage_usage();
        let now = Nanoseconds::near_timestamp();

        let update = self.verify(&payload.0, now);

        // Reject oversized bundles up front (before any storage write) so the eventual NEP-297
        // `UpdatePrices` log — which carries full `FeedData` per feed — stays within NEAR's
        // total-log-length limit. `updated_feeds` only ever shrinks relative to `update.feeds`
        // (some feeds skip on intrinsic-invalidity or anti-replay), so bounding the bundle bounds
        // the event.
        if update.feeds.len() as u64 > u64::from(self.config.max_feeds_per_update) {
            env::panic_str("too many feeds in a single update");
        }

        let mut updated_feeds = Vec::new();
        for feed in &update.feeds {
            // Intrinsic validity lives in `feed_data_from_parsed`; `None` skips. Timestamps are
            // already `Nanoseconds` (converted once in the verifier).
            let Some(feed_data) = feed_data_from_parsed(
                &parsed_feed_view(feed),
                update.timestamp,
                now,
                self.config.max_timestamp_ahead_s,
            ) else {
                continue;
            };

            // Anti-replay (relational, so it stays here): the effective publish timestamp must
            // strictly advance for this feed.
            if let Some(existing) = self.feeds.get(&feed.feed_id) {
                if feed_data.publish_time_ns <= existing.publish_time_ns {
                    continue;
                }
            }

            // Every verified feed is stored by its native Lazer `u32` id; the caller funds the
            // storage (below).
            self.feeds.insert(feed.feed_id, feed_data.clone());
            updated_feeds.push((feed.feed_id, feed_data));
        }

        // Commit pending writes so the storage delta reflects this call's insertions (the helper
        // measures actual storage usage and does not see `store` collections' cached writes), then
        // charge the submitter for the new storage plus the configured update fee and refund any
        // excess.
        self.feeds.flush();
        let _refund =
            apply_storage_fee_and_refund(storage_before, self.config.update_fee.as_yoctonear());

        PythLazerEvent::UpdatePrices { updated_feeds }.emit();
    }

    /// Raw stored data for a Lazer feed id.
    pub fn get_feed_data(&self, feed_id: u32) -> Option<FeedData> {
        self.feeds.get(&feed_id).cloned()
    }

    /// Raw stored data for each requested Lazer feed id (`None` for feeds with no stored data). This
    /// is the adapter's read ABI: consumers (the proxy-oracle's `Lazer` source, the gateway) receive
    /// the raw [`FeedData`] and project it to a Pyth price themselves, applying their own freshness
    /// policy — the adapter stores and serves, it does not transform.
    pub fn get_feeds_data(&self, feed_ids: Vec<u32>) -> FeedDataResponse {
        feed_ids
            .into_iter()
            .map(|feed_id| (feed_id, self.feeds.get(&feed_id).cloned()))
            .collect()
    }

    /// Stateless verify-and-return (read-only): verify a Pyth Lazer solana-format payload against
    /// the configured signer set + freshness/channel policy and return the **full** verified update
    /// (all Lazer properties), **without** writing storage or charging a fee. Panics if
    /// verification fails. This is the official-Lazer-style parity surface; on
    /// NEAR it is most useful called directly via RPC (`near view`) by off-chain clients, or by
    /// on-chain callers through a cross-contract call + callback (NEAR has no sync read calls).
    pub fn verify_update(&self, payload: Base64VecU8) -> VerifiedUpdateView {
        let now = Nanoseconds::near_timestamp();
        verified_update_view(self.verify(&payload.0, now))
    }

    /// Shared verification: build the kernel signer slice from config and run the verifier (panics
    /// on failure). Used by both the storing `update_price_feeds` and the read-only `verify_update`.
    fn verify(&self, raw: &[u8], now: Nanoseconds) -> verifier::VerifiedUpdate {
        let signers = self.config.signers.verifier_signers();

        let params = verifier::VerifyParams {
            trusted_signers: &signers,
            now_s: now.as_secs(),
            max_timestamp_delay_s: self.config.max_timestamp_delay_s,
            max_timestamp_ahead_s: self.config.max_timestamp_ahead_s,
            allowed_channel_id: self.config.allowed_channel_id,
        };

        verifier::verify_solana_update(&EnvCrypto, raw, &params).unwrap_or_reject()
    }
}
