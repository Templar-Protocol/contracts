//! Property tests for the feed updater on the real contract.
//!
//! `pushes_and_owner_changes_match_the_reference_model` runs generated
//! sequences of pushes (duplicate feeds, missing fields, non-positive prices,
//! every channel, window-edge and replayed timestamps, verifier rejections and
//! traps), clock moves, freshness/registry/verifier changes, epoch resets and
//! unauthorized owner calls. A small reference model says what must be stored;
//! after every step the stored prices, the raw price map, the registry, the
//! epoch, the config, the push's event and every SEP-40 read are compared with
//! it.
//!
//! `malformed_payloads_never_corrupt_state` pushes arbitrary, bit-flipped,
//! truncated and extended payloads: a push either fails without any write or
//! stores only prices the ingest rules admit.
//!
//! Servability is `normalized_to_sep40` from the audited common crate. Set
//! `PROPTEST_CASES` to scale a run (default 128 cases per property).

extern crate std;

use std::{collections::BTreeMap, vec::Vec as StdVec};

use proptest::prelude::*;
use soroban_sdk::{
    testutils::{Address as _, EnvTestConfig, Events as _, Ledger as _},
    Address, BytesN, Env, Event as _, Map, Vec,
};

use super::*;
use crate::testutils::{
    encode_payload, FeedSpec, MockVerifier, MockVerifierClient, PYTH_NOT_INITIALIZED,
    PYTH_SIGNER_EXPIRED, PYTH_SIGNER_NOT_TRUSTED,
};

/// Feed ids pushes draw from; registries are subsets of it.
const UNIVERSE: [u32; 6] = [0, 1, 2, 23, 112, u32::MAX];
const START_SECS: u64 = 1_771_252_171;
const AGES: [u64; 3] = [1, 60, MAX_INGEST_AGE_SECS];
const DRIFTS: [u64; 3] = [0, 5, MAX_INGEST_CLOCK_DRIFT_SECS];
const DEFAULT_CASES: u32 = 128;

fn proptest_config() -> ProptestConfig {
    let mut config = ProptestConfig::default();
    if std::env::var_os("PROPTEST_CASES").is_none() {
        config.cases = DEFAULT_CASES;
    }
    config
}

#[derive(Clone, Debug)]
enum Verdict {
    Accept,
    Reject(u32),
    Trap,
}

#[derive(Clone, Debug)]
struct GeneratedFeed {
    feed_id: u32,
    price: Option<i64>,
    exponent: Option<i16>,
    /// Update time relative to ledger time.
    offset_us: Option<i64>,
}

#[derive(Clone, Debug)]
enum AdminCall {
    Freshness,
    Registry,
    Verification,
    ResetEpoch,
    Upgrade,
}

#[derive(Clone, Debug)]
enum Step {
    Advance(u64),
    Push {
        channel: LazerChannel,
        feeds: StdVec<GeneratedFeed>,
        verdict: Verdict,
    },
    SetFreshness(FreshnessConfig),
    SetRegistry(StdVec<u32>),
    Rotate {
        new_verifier: bool,
        channel: LazerChannel,
    },
    ResetEpoch,
    Unauthorized(AdminCall),
}

fn channel_code(channel: LazerChannel) -> u8 {
    match channel {
        LazerChannel::RealTime => 1,
        LazerChannel::FixedRate50ms => 2,
        LazerChannel::FixedRate200ms => 3,
        LazerChannel::FixedRate1000ms => 4,
    }
}

fn channel() -> impl Strategy<Value = LazerChannel> {
    prop_oneof![
        Just(LazerChannel::RealTime),
        Just(LazerChannel::FixedRate50ms),
        Just(LazerChannel::FixedRate200ms),
        Just(LazerChannel::FixedRate1000ms),
    ]
}

fn price() -> impl Strategy<Value = Option<i64>> {
    prop_oneof![
        1 => Just(None),
        1 => Just(Some(0)),
        1 => prop::sample::select(&[-1_i64, i64::MIN][..]).prop_map(Some),
        1 => Just(Some(i64::MAX)),
        8 => (1_i64..=1_000_000_000_000_000).prop_map(Some),
    ]
}

fn exponent() -> impl Strategy<Value = Option<i16>> {
    prop_oneof![
        1 => Just(None),
        1 => prop::sample::select(&[i16::MIN, -40, -19, 19, i16::MAX][..]).prop_map(Some),
        8 => (-14_i16..=2).prop_map(Some),
    ]
}

/// Offsets at every window edge the generated configs can produce, plus
/// a spread across a week either side.
fn offset_us() -> impl Strategy<Value = Option<i64>> {
    let micros = i64::try_from(MICROS_PER_SEC).expect("fits");
    let mut edges = std::vec![-1_i64, 0, 1];
    for age in AGES {
        let age_us = i64::try_from(age).expect("fits") * micros;
        edges.extend([-age_us, -age_us - 1]);
    }
    for drift in DRIFTS {
        let drift_us = i64::try_from(drift).expect("fits") * micros;
        edges.extend([drift_us, drift_us + 1]);
    }
    prop_oneof![
        1 => Just(None),
        4 => prop::sample::select(edges).prop_map(Some),
        4 => (-2_000_000_i64..=2_000_000).prop_map(Some),
        2 => (-700_000 * micros..=120 * micros).prop_map(Some),
    ]
}

fn generated_feed() -> impl Strategy<Value = GeneratedFeed> {
    (
        prop::sample::select(&UNIVERSE[..]),
        price(),
        exponent(),
        offset_us(),
    )
        .prop_map(|(feed_id, price, exponent, offset_us)| GeneratedFeed {
            feed_id,
            price,
            exponent,
            offset_us,
        })
}

fn verdict() -> impl Strategy<Value = Verdict> {
    prop_oneof![
        12 => Just(Verdict::Accept),
        1 => prop::sample::select(
            &[PYTH_SIGNER_NOT_TRUSTED, PYTH_SIGNER_EXPIRED, PYTH_NOT_INITIALIZED][..]
        )
        .prop_map(Verdict::Reject),
        1 => Just(Verdict::Trap),
    ]
}

fn freshness() -> impl Strategy<Value = FreshnessConfig> {
    (
        prop::sample::select(&[0, 1, 60, MAX_INGEST_AGE_SECS, MAX_INGEST_AGE_SECS + 1][..]),
        prop::sample::select(
            &[
                0,
                5,
                MAX_INGEST_CLOCK_DRIFT_SECS,
                MAX_INGEST_CLOCK_DRIFT_SECS + 1,
            ][..],
        ),
    )
        .prop_map(|(max_age_secs, max_clock_drift_secs)| FreshnessConfig {
            max_age_secs,
            max_clock_drift_secs,
        })
}

/// A subset of the universe, sometimes made invalid: unsorted, duplicated,
/// or one past the registry cap.
fn registry() -> impl Strategy<Value = StdVec<u32>> {
    (0_u8..64, 0_u8..8).prop_map(|(mask, mangle)| {
        let mut ids: StdVec<u32> = UNIVERSE
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, id)| *id)
            .collect();
        match mangle {
            0 if ids.len() >= 2 => ids.reverse(),
            1 if !ids.is_empty() => ids.push(ids[0]),
            2 => ids = (0..=MAX_SUPPORTED_FEEDS).collect(),
            _ => {}
        }
        ids
    })
}

fn admin_call() -> impl Strategy<Value = AdminCall> {
    prop_oneof![
        Just(AdminCall::Freshness),
        Just(AdminCall::Registry),
        Just(AdminCall::Verification),
        Just(AdminCall::ResetEpoch),
        Just(AdminCall::Upgrade),
    ]
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        3 => prop_oneof![
            0_u64..=2,
            1_u64..=120,
            Just(MAX_INGEST_AGE_SECS),
            MAX_INGEST_AGE_SECS - 120..=MAX_INGEST_AGE_SECS + 120,
        ]
        .prop_map(Step::Advance),
        8 => (channel(), prop::collection::vec(generated_feed(), 0..8), verdict())
            .prop_map(|(channel, feeds, verdict)| Step::Push { channel, feeds, verdict }),
        1 => freshness().prop_map(Step::SetFreshness),
        1 => registry().prop_map(Step::SetRegistry),
        1 => (any::<bool>(), channel())
            .prop_map(|(new_verifier, channel)| Step::Rotate { new_verifier, channel }),
        1 => Just(Step::ResetEpoch),
        1 => admin_call().prop_map(Step::Unauthorized),
    ]
}

/// A `try_` client call reduced to the contract's own result.
fn outcome<T, C: core::fmt::Debug, I: core::fmt::Debug>(
    result: Result<Result<T, C>, Result<LazerSourceError, I>>,
) -> Result<T, LazerSourceError> {
    match result {
        Ok(Ok(value)) => Ok(value),
        Err(Ok(error)) => Err(error),
        Ok(Err(conversion)) => panic!("unexpected conversion failure {conversion:?}"),
        Err(Err(invoke)) => panic!("unexpected host failure {invoke:?}"),
    }
}

/// What the contract must hold; mirrors the documented ingest rules.
#[derive(Clone, Debug)]
struct Model {
    verifier: Address,
    channel: LazerChannel,
    freshness: FreshnessConfig,
    registry: StdVec<u32>,
    epoch: u64,
    prices: BTreeMap<u32, StoredPrice>,
}

struct World {
    env: Env,
    owner: Address,
    source: PythLazerSourceClient<'static>,
    base: Asset,
    decimals: u32,
    model: Model,
}

impl World {
    fn new(decimals: u32, channel: LazerChannel) -> Self {
        let env = Env::new_with_config(EnvTestConfig {
            capture_snapshot_at_drop: false,
        });
        env.mock_all_auths();
        env.ledger().set_timestamp(START_SECS);
        let owner = Address::generate(&env);
        let verifier = env.register(MockVerifier, ());
        let base = Asset::Other(Symbol::new(&env, "USD"));
        let freshness = FreshnessConfig {
            max_age_secs: 60,
            max_clock_drift_secs: 5,
        };
        let registry = std::vec![1, 2, 23];
        let source_id = env.register(
            PythLazerSource,
            (
                &owner,
                Config {
                    verifier: verifier.clone(),
                    base: base.clone(),
                    decimals,
                    channel,
                    freshness: freshness.clone(),
                },
                Vec::from_slice(&env, &registry),
            ),
        );
        let source = PythLazerSourceClient::new(&env, &source_id);
        Self {
            env,
            owner,
            source,
            base,
            decimals,
            model: Model {
                verifier,
                channel,
                freshness,
                registry,
                epoch: 0,
                prices: BTreeMap::new(),
            },
        }
    }

    fn now_us(&self) -> u64 {
        self.env.ledger().timestamp() * MICROS_PER_SEC
    }

    fn feed_specs(&self, feeds: &[GeneratedFeed]) -> StdVec<FeedSpec> {
        let now_us = self.now_us();
        feeds
            .iter()
            .map(|feed| FeedSpec {
                feed_id: feed.feed_id,
                price: feed.price,
                exponent: feed.exponent,
                feed_update_timestamp: feed
                    .offset_us
                    .map(|offset| now_us.checked_add_signed(offset).expect("in range")),
            })
            .collect()
    }

    fn servable(&self, price: &StoredPrice) -> Option<PriceData> {
        normalized_to_sep40(
            &NormalizedPrice {
                mantissa: price.mantissa,
                expo: price.expo,
                timestamp: price.publish_time_us / MICROS_PER_SEC,
            },
            self.decimals,
        )
        .ok()
    }

    /// Inclusive ingest window in exact arithmetic.
    fn window(&self) -> (u128, u128) {
        let micros = u128::from(MICROS_PER_SEC);
        let now_us = u128::from(self.env.ledger().timestamp()) * micros;
        (
            now_us.saturating_sub(u128::from(self.model.freshness.max_age_secs) * micros),
            now_us + u128::from(self.model.freshness.max_clock_drift_secs) * micros,
        )
    }

    /// The model's push: which feeds are stored and the event it emits.
    fn model_push(&mut self, specs: &[FeedSpec]) -> Option<PricesUpdated> {
        let (oldest, latest) = self.window();
        let mut event = PricesUpdated {
            epoch: self.model.epoch,
            feed_ids: Vec::new(&self.env),
            mantissas: Vec::new(&self.env),
            expos: Vec::new(&self.env),
            publish_times_us: Vec::new(&self.env),
        };
        for spec in specs {
            if !self.model.registry.contains(&spec.feed_id) {
                continue;
            }
            let (Some(mantissa), Some(exponent), Some(time)) =
                (spec.price, spec.exponent, spec.feed_update_timestamp)
            else {
                continue;
            };
            let advances = self
                .model
                .prices
                .get(&spec.feed_id)
                .is_none_or(|stored| time > stored.publish_time_us);
            let in_window = (oldest..=latest).contains(&u128::from(time));
            let candidate = StoredPrice {
                mantissa,
                expo: i32::from(exponent),
                publish_time_us: time,
            };
            if mantissa <= 0 || !in_window || !advances || self.servable(&candidate).is_none() {
                continue;
            }
            event.feed_ids.push_back(spec.feed_id);
            event.mantissas.push_back(candidate.mantissa);
            event.expos.push_back(candidate.expo);
            event.publish_times_us.push_back(candidate.publish_time_us);
            self.model.prices.insert(spec.feed_id, candidate);
        }
        (!event.feed_ids.is_empty()).then_some(event)
    }

    fn push(&mut self, channel: LazerChannel, feeds: &[GeneratedFeed], verdict: &Verdict) {
        let verifier = MockVerifierClient::new(&self.env, &self.model.verifier);
        let (reject, trap) = match verdict {
            Verdict::Accept => (0, false),
            Verdict::Reject(code) => (*code, false),
            Verdict::Trap => (0, true),
        };
        verifier.set_reject(&reject);
        verifier.set_trap(&trap);
        let specs = self.feed_specs(feeds);
        let payload = encode_payload(&self.env, self.now_us(), channel_code(channel), &specs);

        let result = outcome(self.source.try_update_price_feeds(&payload));
        let events = self
            .env
            .events()
            .all()
            .filter_by_contract(&self.source.address);

        let refused = match verdict {
            Verdict::Trap => Some(LazerSourceError::VerifierInvokeFailed),
            Verdict::Reject(code) if *code == PYTH_NOT_INITIALIZED => {
                Some(LazerSourceError::VerifierInvokeFailed)
            }
            Verdict::Reject(_) => Some(LazerSourceError::VerifierRejected),
            Verdict::Accept if channel != self.model.channel => {
                Some(LazerSourceError::ChannelMismatch)
            }
            Verdict::Accept => None,
        };
        if let Some(error) = refused {
            assert_eq!(result, Err(error));
            assert_eq!(events.events().len(), 0, "a failed push emitted");
            return;
        }
        let event = self.model_push(&specs);
        let count = event.as_ref().map_or(0, |event| event.feed_ids.len());
        assert_eq!(result, Ok(count));
        match event {
            Some(event) => assert_eq!(
                events,
                std::vec![event.to_xdr(&self.env, &self.source.address)]
            ),
            None => assert_eq!(events.events().len(), 0, "an empty push emitted"),
        }
    }

    fn set_freshness(&mut self, freshness: FreshnessConfig) {
        let result = outcome(self.source.try_set_freshness(&freshness));
        if validate_freshness(&freshness).is_ok() {
            assert_eq!(result, Ok(()));
            self.model.freshness = freshness;
        } else {
            assert_eq!(result, Err(LazerSourceError::InvalidInput));
        }
    }

    fn set_registry(&mut self, ids: StdVec<u32>) {
        let result = outcome(
            self.source
                .try_set_supported_feed_ids(&Vec::from_slice(&self.env, &ids)),
        );
        let valid = !ids.is_empty()
            && ids.len() <= MAX_SUPPORTED_FEEDS as usize
            && ids.windows(2).all(|pair| pair[0] < pair[1]);
        if valid {
            assert_eq!(result, Ok(()));
            self.model.prices.retain(|id, _| ids.contains(id));
            self.model.registry = ids;
        } else {
            assert_eq!(result, Err(LazerSourceError::InvalidInput));
        }
    }

    fn rotate(&mut self, new_verifier: bool, channel: LazerChannel) {
        let verifier = if new_verifier {
            self.env.register(MockVerifier, ())
        } else {
            self.model.verifier.clone()
        };
        let result = outcome(self.source.try_set_verification_config(&verifier, &channel));
        assert_eq!(result, Ok(()));
        if new_verifier || channel != self.model.channel {
            self.model.verifier = verifier;
            self.model.channel = channel;
            self.start_epoch();
        }
    }

    fn start_epoch(&mut self) {
        self.model.epoch += 1;
        self.model.prices.clear();
    }

    fn unauthorized(&mut self, call: &AdminCall) {
        self.env.mock_auths(&[]);
        let failed = match call {
            AdminCall::Freshness => self
                .source
                .try_set_freshness(&FreshnessConfig {
                    max_age_secs: 1,
                    max_clock_drift_secs: 0,
                })
                .is_err(),
            AdminCall::Registry => self
                .source
                .try_set_supported_feed_ids(&Vec::from_array(&self.env, [UNIVERSE[0]]))
                .is_err(),
            AdminCall::Verification => self
                .source
                .try_set_verification_config(&Address::generate(&self.env), &LazerChannel::RealTime)
                .is_err(),
            AdminCall::ResetEpoch => self.source.try_reset_verification_epoch().is_err(),
            AdminCall::Upgrade => self
                .source
                .try_upgrade(&BytesN::from_array(&self.env, &[7; 32]), &self.owner)
                .is_err(),
        };
        self.env.mock_all_auths();
        assert!(failed, "{call:?} ran without the owner's authorization");
    }

    fn apply(&mut self, step: &Step) {
        match step {
            Step::Advance(seconds) => {
                let now = self.env.ledger().timestamp();
                self.env.ledger().set_timestamp(now + seconds);
            }
            Step::Push {
                channel,
                feeds,
                verdict,
            } => self.push(*channel, feeds, verdict),
            Step::SetFreshness(freshness) => self.set_freshness(freshness.clone()),
            Step::SetRegistry(ids) => self.set_registry(ids.clone()),
            Step::Rotate {
                new_verifier,
                channel,
            } => self.rotate(*new_verifier, *channel),
            Step::ResetEpoch => {
                assert_eq!(outcome(self.source.try_reset_verification_epoch()), Ok(()));
                self.start_epoch();
            }
            Step::Unauthorized(call) => self.unauthorized(call),
        }
    }

    fn raw_prices(&self) -> Option<Map<u32, StoredPrice>> {
        self.env.as_contract(&self.source.address, || {
            self.env.storage().persistent().get(&PRICES)
        })
    }

    /// Every observable read agrees with the model.
    fn assert_matches_model(&self) {
        let model = &self.model;
        assert_eq!(self.source.verification_epoch(), model.epoch);
        assert_eq!(
            self.source.supported_feed_ids(),
            Vec::from_slice(&self.env, &model.registry)
        );
        assert_eq!(
            self.source.config(),
            Some(Config {
                verifier: model.verifier.clone(),
                base: self.base.clone(),
                decimals: self.decimals,
                channel: model.channel,
                freshness: model.freshness.clone(),
            })
        );
        let mut assets = Vec::new(&self.env);
        for id in &model.registry {
            assets.push_back(feed_asset(&self.env, *id));
        }
        assert_eq!(self.source.assets(), assets);

        // The raw map holds exactly the modelled prices: no orphan survives a
        // registry change or an epoch, and an empty map is removed.
        match self.raw_prices() {
            None => assert!(model.prices.is_empty()),
            Some(raw) => {
                let raw: BTreeMap<u32, StoredPrice> = raw.iter().collect();
                assert_eq!(raw, model.prices);
                assert!(!raw.is_empty(), "an empty price map was stored");
            }
        }

        for id in UNIVERSE {
            let asset = feed_asset(&self.env, id);
            let stored = model.prices.get(&id);
            assert_eq!(self.source.stored_price(&id).as_ref(), stored);
            // Storing means servable: every stored price is readable.
            let expected = stored.map(|price| self.servable(price).expect("stored is servable"));
            let last = self.source.lastprice(&asset);
            assert_eq!(last, expected);
            assert_eq!(self.source.prices(&asset, &0), None);
            assert_eq!(
                self.source.prices(&asset, &3),
                last.clone()
                    .map(|price| Vec::from_array(&self.env, [price]))
            );
            if let Some(price) = last {
                assert_eq!(
                    self.source.price(&asset, &price.timestamp),
                    Some(price.clone())
                );
                assert_eq!(self.source.price(&asset, &(price.timestamp + 1)), None);
                assert!(model.registry.contains(&id));
                assert!(price.price > 0);
            }
        }
        assert_eq!(
            self.source
                .lastprice(&Asset::Other(Symbol::new(&self.env, "01"))),
            None,
            "a non-canonical key resolved"
        );
    }
}

proptest! {
    #![proptest_config(proptest_config())]

    #[test]
    fn pushes_and_owner_changes_match_the_reference_model(
        decimals in 0..=MAX_SUPPORTED_SEP40_DECIMALS,
        initial_channel in channel(),
        steps in prop::collection::vec(step(), 1..40),
    ) {
        let mut world = World::new(decimals, initial_channel);
        world.assert_matches_model();
        for step in &steps {
            world.apply(step);
            world.assert_matches_model();
        }
    }

    #[test]
    fn malformed_payloads_never_corrupt_state(
        decimals in 0..=MAX_SUPPORTED_SEP40_DECIMALS,
        seed in prop::collection::vec(generated_feed(), 1..6),
        feeds in prop::collection::vec(generated_feed(), 0..6),
        mutation in prop_oneof![
            prop::collection::vec(any::<u8>(), 0..300).prop_map(Mutation::Replace),
            prop::collection::vec((any::<prop::sample::Index>(), 1_u8..=255), 1..4)
                .prop_map(Mutation::Flip),
            any::<prop::sample::Index>().prop_map(Mutation::Truncate),
            prop::collection::vec(any::<u8>(), 1..40).prop_map(Mutation::Extend),
        ],
    ) {
        let mut world = World::new(decimals, LazerChannel::FixedRate200ms);
        world.apply(&Step::Push {
            channel: LazerChannel::FixedRate200ms,
            feeds: seed,
            verdict: Verdict::Accept,
        });
        world.assert_matches_model();
        let before = world.model.prices.clone();

        let specs = world.feed_specs(&feeds);
        let valid = encode_payload(
            &world.env,
            world.now_us(),
            channel_code(LazerChannel::FixedRate200ms),
            &specs,
        );
        let mut bytes: StdVec<u8> = valid.iter().collect();
        mutation.apply(&mut bytes);
        let payload = soroban_sdk::Bytes::from_slice(&world.env, &bytes);

        let result = outcome(world.source.try_update_price_feeds(&payload));
        let emitted = world
            .env
            .events()
            .all()
            .filter_by_contract(&world.source.address)
            .events()
            .len();
        let after: BTreeMap<u32, StoredPrice> = world
            .raw_prices()
            .map(|raw| raw.iter().collect())
            .unwrap_or_default();

        match result {
            Err(error) => {
                prop_assert!(matches!(
                    error,
                    LazerSourceError::InvalidPayload | LazerSourceError::ChannelMismatch
                ));
                prop_assert_eq!(&after, &before);
                prop_assert_eq!(emitted, 0);
            }
            Ok(count) => {
                let (oldest, latest) = world.window();
                let mut changed = 0_u32;
                for (id, price) in &after {
                    let previous = before.get(id);
                    if previous == Some(price) {
                        continue;
                    }
                    changed += 1;
                    prop_assert!(world.model.registry.contains(id));
                    prop_assert!(price.mantissa > 0);
                    prop_assert!((oldest..=latest).contains(&u128::from(price.publish_time_us)));
                    prop_assert!(previous.is_none_or(|p| price.publish_time_us > p.publish_time_us));
                    prop_assert!(world.servable(price).is_some());
                }
                // A push never removes a price.
                prop_assert!(before.keys().all(|id| after.contains_key(id)));
                prop_assert!(count >= changed);
                prop_assert_eq!(count == 0, changed == 0);
                prop_assert_eq!(emitted, usize::from(count > 0));
            }
        }
    }
}

#[derive(Clone, Debug)]
enum Mutation {
    Replace(StdVec<u8>),
    Flip(StdVec<(prop::sample::Index, u8)>),
    Truncate(prop::sample::Index),
    Extend(StdVec<u8>),
}

impl Mutation {
    fn apply(&self, bytes: &mut StdVec<u8>) {
        match self {
            Self::Replace(replacement) => bytes.clone_from(replacement),
            Self::Flip(flips) => {
                for (index, mask) in flips {
                    let at = index.index(bytes.len());
                    bytes[at] ^= mask;
                }
            }
            Self::Truncate(index) => bytes.truncate(index.index(bytes.len())),
            Self::Extend(tail) => bytes.extend_from_slice(tail),
        }
    }
}
