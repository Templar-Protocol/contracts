#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::should_panic_without_expect)]

use super::*;

use rstest::rstest;
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger, MockAuth, MockAuthInvoke},
    Address, Bytes, BytesN, Env, IntoVal, InvokeError, Symbol, Val, Vec,
};

use crate::testutils::{
    encode_payload, feed, payload_at, FeedSpec, MockVerifier, MockVerifierClient,
    MockVerifierError, CHANNEL_200MS, CHANNEL_REAL_TIME,
};

/// Pyth's shared Sui/Stellar test vector: BTC (1), ETH (2), SOL (112) on
/// `fixed_rate@200ms` at 1_771_252_161_800_000 µs.
const VECTOR_TIMESTAMP_US: u64 = 1_771_252_161_800_000;
const VECTOR_BTC_PRICE: i64 = 6_828_284_601_313;
const VECTOR_ETH_PRICE: i64 = 195_892_878_231;

static OVERSIZED_PAYLOAD: [u8; MAX_LAZER_ENVELOPE_BYTES as usize + 1] =
    [0; MAX_LAZER_ENVELOPE_BYTES as usize + 1];
fn vector_payload(env: &Env) -> Bytes {
    Bytes::from_slice(
        env,
        &hex_literal::hex!(
            "75d3c7934067e9c7f14a06000303010000000b00e1637ad5"
            "35060000015a2507d335060000027f8bfdf53506000004f8"
            "ff0600070008000900000a601299cd3e0600000bc07595c7"
            "3e0600000c014067e9c7f14a0600020000000b00971b209c"
            "2d0000000144056b9b2d0000000298fb6b9c2d00000004f8"
            "ff0600070008000900000a284444f92d0000000b480c07f9"
            "2d0000000c014067e9c7f14a0600700000000b0020d85dd2"
            "d78df30001000000000000000002000000000000000004f4"
            "ff060130f80bfeffffffff0701b8ab7057ec4a0600080100"
            "209db4060000000900000a00000000000000000b00000000"
            "000000000c014067e9c7f14a0600"
        ),
    )
}

const BTC_FEED: u32 = 1;
const ETH_FEED: u32 = 2;
const SOL_FEED: u32 = 112;

struct Harness {
    env: Env,
    owner: Address,
    verifier: MockVerifierClient<'static>,
    source: PythLazerSourceClient<'static>,
    base: Asset,
    btc: Asset,
    eth: Asset,
}

fn freshness() -> FreshnessConfig {
    FreshnessConfig {
        max_age_secs: 60,
        max_clock_drift_secs: 5,
    }
}

fn symbol_asset(env: &Env, symbol: &str) -> Asset {
    Asset::Other(Symbol::new(env, symbol))
}

fn config(env: &Env, channel: LazerChannel, decimals: u32, max_age_secs: u64) -> Config {
    Config {
        verifier: env.register(MockVerifier, ()),
        base: symbol_asset(env, "USD"),
        decimals,
        channel,
        freshness: FreshnessConfig {
            max_age_secs,
            max_clock_drift_secs: 5,
        },
    }
}

fn harness_with(channel: LazerChannel) -> Harness {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger()
        .set_timestamp(VECTOR_TIMESTAMP_US / MICROS_PER_SEC + 10);
    let owner = Address::generate(&env);
    let config = config(&env, channel, 8, 60);
    let verifier_id = config.verifier.clone();
    let base = config.base.clone();
    let source_id = env.register(
        PythLazerSource,
        (&owner, config, Vec::from_array(&env, [BTC_FEED, ETH_FEED])),
    );
    Harness {
        verifier: MockVerifierClient::new(&env, &verifier_id),
        source: PythLazerSourceClient::new(&env, &source_id),
        btc: feed_asset(&env, BTC_FEED),
        eth: feed_asset(&env, ETH_FEED),
        env,
        owner,
        base,
    }
}

fn harness() -> Harness {
    harness_with(LazerChannel::FixedRate200ms)
}

fn mock_contract_auth(h: &Harness, address: &Address, fn_name: &'static str, args: Vec<Val>) {
    let invoke = MockAuthInvoke {
        contract: &h.source.address,
        fn_name,
        args,
        sub_invokes: &[],
    };
    h.env.mock_auths(&[MockAuth {
        address,
        invoke: &invoke,
    }]);
}

fn stored_btc(h: &Harness) -> StoredPrice {
    h.source.stored_price(&BTC_FEED).expect("btc stored")
}

fn construct(decimals: u32, max_age_secs: u64) {
    let env = Env::default();
    let owner = Address::generate(&env);
    let config = config(&env, LazerChannel::FixedRate200ms, decimals, max_age_secs);
    env.register(
        PythLazerSource,
        (&owner, config, Vec::from_array(&env, [BTC_FEED])),
    );
}

#[rstest]
#[should_panic]
#[case::decimals_above_max(19, 60)]
#[should_panic]
#[case::zero_max_age(8, 0)]
fn constructor_rejects_invalid_config(#[case] decimals: u32, #[case] max_age_secs: u64) {
    construct(decimals, max_age_secs);
}

#[test]
fn constructor_accepts_boundary_config() {
    construct(MAX_SUPPORTED_SEP40_DECIMALS, 1);
}

#[rstest]
#[case(0, "0")]
#[case(7, "7")]
#[case(23, "23")]
#[case(240, "240")]
#[case(u32::MAX, "4294967295")]
fn feed_asset_is_the_decimal_id(#[case] feed_id: u32, #[case] text: &str) {
    let env = Env::default();
    assert_eq!(
        feed_asset(&env, feed_id),
        Asset::Other(Symbol::new(&env, text))
    );
}

#[test]
fn exposes_sep40_metadata_and_config() {
    let h = harness();
    assert_eq!(h.source.base(), h.base);
    assert_eq!(h.source.decimals(), 8);
    assert_eq!(h.source.resolution(), 1);
    assert_eq!(
        h.source.assets(),
        Vec::from_array(&h.env, [h.btc.clone(), h.eth.clone()])
    );
    let config = h.source.config().expect("config");
    assert_eq!(config.channel, LazerChannel::FixedRate200ms);
    assert_eq!(config.freshness, freshness());
    assert_eq!(h.source.get_owner(), Some(h.owner.clone()));
    assert_eq!(h.source.lastprice(&h.btc), None);
}

#[test]
fn stores_only_registered_feeds_in_the_pyth_vector_under_native_ids() {
    let h = harness();
    assert_eq!(h.source.update_price_feeds(&vector_payload(&h.env)), 2);
    let updates = h.env.events().all().filter_by_contract(&h.source.address);
    assert_eq!(updates.events().len(), 2);

    assert_eq!(
        h.source.lastprice(&h.btc),
        Some(PriceData {
            price: i128::from(VECTOR_BTC_PRICE),
            timestamp: VECTOR_TIMESTAMP_US / MICROS_PER_SEC,
        })
    );
    assert_eq!(
        h.source.lastprice(&h.eth).map(|p| p.price),
        Some(i128::from(VECTOR_ETH_PRICE))
    );
    assert_eq!(h.source.stored_price(&SOL_FEED), None);
    assert_eq!(
        stored_btc(&h),
        StoredPrice {
            mantissa: VECTOR_BTC_PRICE,
            expo: -8,
            publish_time_us: VECTOR_TIMESTAMP_US,
        }
    );
    assert_eq!(h.source.lastprice(&symbol_asset(&h.env, "BTC")), None);
    for alias in ["01", "001", "1_"] {
        assert_eq!(
            h.source.lastprice(&symbol_asset(&h.env, alias)),
            None,
            "accepted noncanonical alias {alias}"
        );
    }
}

#[test]
fn rejects_channel_mismatch() {
    let h = harness_with(LazerChannel::RealTime);
    assert_eq!(
        h.source.try_update_price_feeds(&vector_payload(&h.env)),
        Err(Ok(LazerSourceError::ChannelMismatch))
    );
    let real_time = encode_payload(
        &h.env,
        VECTOR_TIMESTAMP_US,
        CHANNEL_REAL_TIME,
        &[feed(BTC_FEED, 5, VECTOR_TIMESTAMP_US)],
    );
    assert_eq!(h.source.update_price_feeds(&real_time), 1);
}

/// Window is 60 seconds back through 5 seconds of clock drift ahead, checked in µs.
#[rstest]
#[case::too_old(-61_000_000, 0)]
#[case::oldest(-60_000_000, 1)]
#[case::newest(5_000_000, 1)]
#[case::just_ahead(5_000_001, 0)]
#[case::ahead(6_000_000, 0)]
fn feeds_outside_the_window_are_skipped_not_rejected(#[case] offset_us: i64, #[case] stored: u32) {
    let h = harness();
    let now_us = h.env.ledger().timestamp() * MICROS_PER_SEC;
    let at_us = now_us.checked_add_signed(offset_us).expect("in range");
    assert_eq!(
        h.source
            .update_price_feeds(&payload_at(&h.env, at_us, &[(BTC_FEED, 5)])),
        stored
    );
    assert_eq!(h.source.lastprice(&h.btc).is_some(), stored == 1);
}

#[test]
fn publish_time_must_strictly_advance_per_feed() {
    let h = harness();
    let vector = vector_payload(&h.env);
    assert_eq!(h.source.update_price_feeds(&vector), 2);
    assert_eq!(h.source.update_price_feeds(&vector), 0);

    let older = payload_at(&h.env, VECTOR_TIMESTAMP_US - 1, &[(BTC_FEED, 7)]);
    assert_eq!(h.source.update_price_feeds(&older), 0);
    assert_eq!(stored_btc(&h).mantissa, VECTOR_BTC_PRICE);

    let newer = payload_at(&h.env, VECTOR_TIMESTAMP_US + 1, &[(BTC_FEED, 7)]);
    assert_eq!(h.source.update_price_feeds(&newer), 1);
    assert_eq!(stored_btc(&h).mantissa, 7);
    assert_eq!(stored_btc(&h).publish_time_us, VECTOR_TIMESTAMP_US + 1);
}

#[test]
fn replay_does_not_write_unchanged_storage() {
    let h = harness();
    let vector = vector_payload(&h.env);
    assert_eq!(h.source.update_price_feeds(&vector), 2);

    h.env.cost_estimate().budget().reset_default();
    assert_eq!(h.source.update_price_feeds(&vector), 0);
    let replay_resources = h.env.cost_estimate().resources();
    assert_eq!(replay_resources.write_entries, 0);

    h.env.cost_estimate().budget().reset_default();
    let newer = payload_at(
        &h.env,
        VECTOR_TIMESTAMP_US + 1,
        &[(BTC_FEED, VECTOR_BTC_PRICE + 1)],
    );
    assert_eq!(h.source.update_price_feeds(&newer), 1);
    assert!(h.env.cost_estimate().resources().write_entries > 0);
}

#[test]
fn feed_update_time_is_the_stored_clock_and_is_window_checked() {
    let h = harness();
    let now = h.env.ledger().timestamp();
    let payload_us = now * MICROS_PER_SEC;
    let at = |spec: FeedSpec| encode_payload(&h.env, payload_us, CHANNEL_200MS, &[spec]);

    let earlier = payload_us - 5_000_000;
    assert_eq!(
        h.source.update_price_feeds(&at(feed(BTC_FEED, 9, earlier))),
        1
    );
    assert_eq!(stored_btc(&h).publish_time_us, earlier);

    let no_feed_time = FeedSpec {
        feed_update_timestamp: None,
        ..feed(ETH_FEED, 11, payload_us)
    };
    assert_eq!(h.source.update_price_feeds(&at(no_feed_time)), 0);

    assert_eq!(h.source.lastprice(&h.eth), None);
    assert_eq!(stored_btc(&h).mantissa, 9);
}

#[test]
fn skips_feeds_without_a_positive_price_or_an_exponent() {
    let h = harness();
    let payload = encode_payload(
        &h.env,
        VECTOR_TIMESTAMP_US,
        CHANNEL_200MS,
        &[
            feed(BTC_FEED, 0, VECTOR_TIMESTAMP_US),
            feed(ETH_FEED, -1, VECTOR_TIMESTAMP_US),
            FeedSpec {
                exponent: None,
                ..feed(BTC_FEED, 5, VECTOR_TIMESTAMP_US)
            },
        ],
    );
    assert_eq!(h.source.update_price_feeds(&payload), 0);
    assert_eq!(h.source.lastprice(&h.btc), None);
    assert_eq!(h.source.lastprice(&h.eth), None);
}

#[test]
fn verifier_rejection_does_not_mutate_an_existing_price() {
    let h = harness();
    assert_eq!(h.source.update_price_feeds(&vector_payload(&h.env)), 2);
    let before = stored_btc(&h);
    h.verifier.set_reject(&true);
    assert_eq!(
        h.source.try_update_price_feeds(&payload_at(
            &h.env,
            VECTOR_TIMESTAMP_US + 1,
            &[(BTC_FEED, VECTOR_BTC_PRICE + 1)],
        )),
        Err(Err(InvokeError::Contract(
            MockVerifierError::SignerNotTrusted as u32,
        )))
    );
    assert_eq!(stored_btc(&h), before);
}

#[test]
fn malformed_verified_bytes_do_not_mutate_an_existing_price() {
    let h = harness();
    assert_eq!(h.source.update_price_feeds(&vector_payload(&h.env)), 2);
    let before = stored_btc(&h);
    let garbage = Bytes::from_slice(&h.env, &[1, 2, 3]);
    assert_eq!(
        h.source.try_update_price_feeds(&garbage),
        Err(Ok(LazerSourceError::InvalidPayload))
    );
    assert_eq!(stored_btc(&h), before);
}

#[test]
fn truncated_and_trailing_verified_payloads_are_rejected_without_mutation() {
    let h = harness();
    assert_eq!(h.source.update_price_feeds(&vector_payload(&h.env)), 2);
    let before = stored_btc(&h);
    let valid = payload_at(
        &h.env,
        VECTOR_TIMESTAMP_US + 1,
        &[(BTC_FEED, VECTOR_BTC_PRICE + 1)],
    );
    let mut raw = valid.to_alloc_vec();
    let trailing = Bytes::from_slice(&h.env, &[raw.as_slice(), &[0]].concat());
    raw.pop();
    let truncated = Bytes::from_slice(&h.env, &raw);

    for payload in [truncated, trailing] {
        assert_eq!(
            h.source.try_update_price_feeds(&payload),
            Err(Ok(LazerSourceError::InvalidPayload))
        );
        assert_eq!(stored_btc(&h), before);
    }
}

#[test]
fn oversized_envelope_is_rejected_before_verification_or_mutation() {
    let h = harness();
    assert_eq!(h.source.update_price_feeds(&vector_payload(&h.env)), 2);
    let before = stored_btc(&h);
    h.verifier.set_reject(&true);
    let oversized = Bytes::from_slice(&h.env, &OVERSIZED_PAYLOAD);

    assert_eq!(
        h.source.try_update_price_feeds(&oversized),
        Err(Ok(LazerSourceError::InvalidPayload))
    );
    assert_eq!(stored_btc(&h), before);
}

#[test]
fn ledger_time_overflow_does_not_mutate_an_existing_price() {
    let h = harness();
    assert_eq!(h.source.update_price_feeds(&vector_payload(&h.env)), 2);
    let before = stored_btc(&h);
    h.env.ledger().set_timestamp(u64::MAX);

    assert_eq!(
        h.source.try_update_price_feeds(&vector_payload(&h.env)),
        Err(Ok(LazerSourceError::ArithmeticOverflow))
    );
    assert_eq!(stored_btc(&h), before);
}

#[test]
fn set_freshness_validates_without_partial_mutation_and_applies() {
    let h = harness();
    let before = h.source.config().expect("config").freshness;
    assert_eq!(
        h.source.try_set_freshness(&FreshnessConfig {
            max_age_secs: 0,
            ..freshness()
        }),
        Err(Ok(LazerSourceError::InvalidInput))
    );
    assert_eq!(h.source.config().expect("config").freshness, before);

    h.source.set_freshness(&FreshnessConfig {
        max_age_secs: 5,
        max_clock_drift_secs: 0,
    });
    assert_eq!(h.source.update_price_feeds(&vector_payload(&h.env)), 0);
    assert_eq!(
        h.source
            .config()
            .expect("config")
            .freshness
            .max_clock_drift_secs,
        0
    );
}

#[test]
fn set_freshness_requires_exact_owner_auth() {
    let h = harness();
    let next = FreshnessConfig {
        max_age_secs: 5,
        max_clock_drift_secs: 0,
    };
    h.env.mock_auths(&[]);
    assert!(h.source.try_set_freshness(&next).is_err());

    let stranger = Address::generate(&h.env);
    mock_contract_auth(&h, &stranger, "set_freshness", (&next,).into_val(&h.env));
    assert!(h.source.try_set_freshness(&next).is_err());

    mock_contract_auth(&h, &h.owner, "set_freshness", (&next,).into_val(&h.env));
    h.source.set_freshness(&next);
    assert_eq!(h.source.config().expect("config").freshness, next);
}

#[test]
fn feed_registry_requires_exact_owner_auth() {
    let h = harness();
    let next = Vec::from_array(&h.env, [BTC_FEED]);
    h.env.mock_auths(&[]);
    assert!(h.source.try_set_supported_feed_ids(&next).is_err());

    let stranger = Address::generate(&h.env);
    mock_contract_auth(
        &h,
        &stranger,
        "set_supported_feed_ids",
        (&next,).into_val(&h.env),
    );
    assert!(h.source.try_set_supported_feed_ids(&next).is_err());

    mock_contract_auth(
        &h,
        &h.owner,
        "set_supported_feed_ids",
        (&next,).into_val(&h.env),
    );
    h.source.set_supported_feed_ids(&next);
    assert_eq!(h.source.supported_feed_ids(), next);
}

#[test]
fn verification_config_requires_exact_owner_auth() {
    let h = harness();
    let verifier = h.env.register(MockVerifier, ());
    let channel = LazerChannel::RealTime;
    h.env.mock_auths(&[]);
    assert!(h
        .source
        .try_set_verification_config(&verifier, &channel)
        .is_err());

    let stranger = Address::generate(&h.env);
    mock_contract_auth(
        &h,
        &stranger,
        "set_verification_config",
        (&verifier, &channel).into_val(&h.env),
    );
    assert!(h
        .source
        .try_set_verification_config(&verifier, &channel)
        .is_err());

    mock_contract_auth(
        &h,
        &h.owner,
        "set_verification_config",
        (&verifier, &channel).into_val(&h.env),
    );
    h.source.set_verification_config(&verifier, &channel);
    assert_eq!(h.source.config().expect("config").verifier, verifier);
}

#[test]
fn epoch_reset_requires_exact_owner_auth() {
    let h = harness();
    h.env.mock_auths(&[]);
    assert!(h.source.try_reset_verification_epoch().is_err());

    let stranger = Address::generate(&h.env);
    mock_contract_auth(&h, &stranger, "reset_verification_epoch", Vec::new(&h.env));
    assert!(h.source.try_reset_verification_epoch().is_err());

    mock_contract_auth(&h, &h.owner, "reset_verification_epoch", Vec::new(&h.env));
    h.source.reset_verification_epoch();
    assert_eq!(h.source.verification_epoch(), 1);
}

#[test]
fn upgrade_requires_exact_owner_auth() {
    let h = harness();
    let zero_hash = BytesN::from_array(&h.env, &[0_u8; 32]);
    h.env.mock_auths(&[]);
    assert!(h.source.try_upgrade(&zero_hash, &h.owner).is_err());

    let stranger = Address::generate(&h.env);
    mock_contract_auth(
        &h,
        &stranger,
        "upgrade",
        (&zero_hash, &stranger).into_val(&h.env),
    );
    assert_eq!(
        h.source.try_upgrade(&zero_hash, &stranger),
        Err(Ok(LazerSourceError::Unauthorized))
    );

    mock_contract_auth(
        &h,
        &h.owner,
        "upgrade",
        (&zero_hash, &h.owner).into_val(&h.env),
    );
    assert_eq!(
        h.source.try_upgrade(&zero_hash, &h.owner),
        Err(Ok(LazerSourceError::InvalidInput))
    );
}

#[test]
fn feed_registry_enforces_shape_and_epoch_watermark_bound_without_partial_mutation() {
    let h = harness();
    let initial = h.source.supported_feed_ids();
    for invalid in [
        Vec::new(&h.env),
        Vec::from_array(&h.env, [BTC_FEED, BTC_FEED]),
        Vec::from_array(&h.env, [ETH_FEED, BTC_FEED]),
    ] {
        assert_eq!(
            h.source.try_set_supported_feed_ids(&invalid),
            Err(Ok(LazerSourceError::InvalidInput))
        );
        assert_eq!(h.source.supported_feed_ids(), initial);
    }

    for start in (0..MAX_REPLAY_WATERMARKS).step_by(MAX_SUPPORTED_FEEDS as usize) {
        let mut feed_ids = Vec::new(&h.env);
        for feed_id in start..(start + MAX_SUPPORTED_FEEDS).min(MAX_REPLAY_WATERMARKS) {
            feed_ids.push_back(feed_id);
        }
        h.source.set_supported_feed_ids(&feed_ids);
    }
    let full = h.source.supported_feed_ids();
    assert_eq!(full.len(), MAX_SUPPORTED_FEEDS);
    assert_eq!(
        h.source
            .try_set_supported_feed_ids(&Vec::from_array(&h.env, [MAX_REPLAY_WATERMARKS],)),
        Err(Ok(LazerSourceError::InvalidInput))
    );
    assert_eq!(h.source.supported_feed_ids(), full);
}

#[test]
fn maximum_feed_update_stays_within_resource_limits() {
    let h = harness();
    let mut feed_ids = Vec::new(&h.env);
    for feed_id in 0..MAX_SUPPORTED_FEEDS {
        feed_ids.push_back(feed_id);
    }
    h.source.set_supported_feed_ids(&feed_ids);
    let feeds: [FeedSpec; MAX_SUPPORTED_FEEDS as usize] = core::array::from_fn(|index| {
        feed(
            u32::try_from(index).expect("supported feeds fit u32"),
            i64::try_from(index + 1).expect("supported prices fit i64"),
            VECTOR_TIMESTAMP_US,
        )
    });
    let payload = encode_payload(&h.env, VECTOR_TIMESTAMP_US, CHANNEL_200MS, &feeds);

    assert_eq!(h.source.update_price_feeds(&payload), MAX_SUPPORTED_FEEDS);
    let mut replacement = Vec::new(&h.env);
    for feed_id in MAX_SUPPORTED_FEEDS..MAX_SUPPORTED_FEEDS * 2 {
        replacement.push_back(feed_id);
    }
    h.source.set_supported_feed_ids(&replacement);
    assert_eq!(h.source.stored_price(&0), None);
    assert_eq!(h.source.stored_price(&(MAX_SUPPORTED_FEEDS - 1)), None);
}

#[test]
fn price_and_prices_serve_only_the_latest_record() {
    let h = harness();
    h.source
        .update_price_feeds(&payload_at(&h.env, VECTOR_TIMESTAMP_US, &[(BTC_FEED, 5)]));
    let last = h.source.lastprice(&h.btc).expect("btc");
    assert_eq!(h.source.price(&h.btc, &last.timestamp), Some(last.clone()));
    assert_eq!(h.source.price(&h.btc, &(last.timestamp - 1)), None);
    assert_eq!(h.source.prices(&h.btc, &0), None);
    assert_eq!(
        h.source.prices(&h.btc, &5),
        Some(Vec::from_array(&h.env, [last]))
    );
}

#[test]
fn removing_and_readding_a_feed_preserves_its_replay_watermark() {
    let h = harness();
    h.source.update_price_feeds(&vector_payload(&h.env));
    h.source
        .set_supported_feed_ids(&Vec::from_array(&h.env, [BTC_FEED]));
    assert_eq!(h.source.lastprice(&h.eth), None);
    h.source
        .set_supported_feed_ids(&Vec::from_array(&h.env, [BTC_FEED, ETH_FEED]));
    assert_eq!(
        h.source.update_price_feeds(&payload_at(
            &h.env,
            VECTOR_TIMESTAMP_US,
            &[(ETH_FEED, VECTOR_ETH_PRICE + 1)],
        )),
        0
    );
    assert_eq!(
        h.source.update_price_feeds(&payload_at(
            &h.env,
            VECTOR_TIMESTAMP_US + 1,
            &[(ETH_FEED, VECTOR_ETH_PRICE + 1)],
        )),
        1
    );
}

#[test]
fn verification_rotation_starts_a_new_epoch_and_clears_prices() {
    let h = harness();
    h.source
        .update_price_feeds(&payload_at(&h.env, VECTOR_TIMESTAMP_US, &[(BTC_FEED, 1)]));
    let replacement = h.env.register(MockVerifier, ());
    h.source
        .set_verification_config(&replacement, &LazerChannel::FixedRate200ms);
    assert_eq!(h.source.verification_epoch(), 1);
    assert_eq!(h.source.lastprice(&h.btc), None);
    assert_eq!(
        h.source
            .update_price_feeds(&payload_at(&h.env, VECTOR_TIMESTAMP_US, &[(BTC_FEED, 2)],)),
        1
    );
}

#[test]
fn updates_reads_and_ttl_extension_are_permissionless() {
    let h = harness();
    h.env.mock_auths(&[]);
    assert_eq!(h.source.update_price_feeds(&vector_payload(&h.env)), 2);
    assert!(h.source.lastprice(&h.btc).is_some());
    assert!(h.source.stored_price(&BTC_FEED).is_some());
    h.source.extend_ttl();
}
