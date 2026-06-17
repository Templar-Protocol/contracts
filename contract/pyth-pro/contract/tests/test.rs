#![allow(clippy::unwrap_used)]

use byteorder::LE;
use ed25519_dalek::{Signer, SigningKey};
use near_sdk::{
    json_types::Base64VecU8,
    mock::MockAction,
    test_utils::{get_created_receipts, VMContextBuilder},
    testing_env, AccountId, NearToken,
};
use pyth_lazer_protocol::message::SolanaMessage;
use pyth_lazer_protocol::payload::{PayloadData, PayloadFeedData, PayloadPropertyValue};
use pyth_lazer_protocol::time::TimestampUs;
use pyth_lazer_protocol::{ChannelId, Price, PriceFeedId};
use templar_common::oracle::pyth::PriceIdentifier;

use templar_pyth_pro_adapter_contract::{ConfigArgs, Contract, TrustedSigner};

const FEED_ID: u32 = 2;
const NOW_S: u64 = 1_700_000_000;
const EXPO: i16 = -8;

fn signing_key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

fn signer_public_key() -> [u8; 32] {
    signing_key().verifying_key().to_bytes()
}

/// Sign a solana-format (ed25519) Lazer message wrapping `data` with the default signer.
fn sign(data: &PayloadData) -> Vec<u8> {
    sign_with(&signing_key(), data)
}

/// Sign a solana-format (ed25519) Lazer message wrapping `data` with an arbitrary key (used to
/// exercise signer rotation).
fn sign_with(key: &SigningKey, data: &PayloadData) -> Vec<u8> {
    let mut payload = Vec::new();
    data.serialize::<LE>(&mut payload).unwrap();

    let message = SolanaMessage {
        signature: key.sign(&payload).to_bytes(),
        public_key: key.verifying_key().to_bytes(),
        payload,
    };
    let mut raw = Vec::new();
    message.serialize(&mut raw).unwrap();
    raw
}

/// A standard single-feed real-time payload signed by `key` (for rotation tests).
fn real_time_signed_by(
    key: &SigningKey,
    timestamp_us: u64,
    price: i64,
    ema: i64,
    conf: i64,
) -> Vec<u8> {
    sign_with(
        key,
        &PayloadData {
            timestamp_us: TimestampUs::from_micros(timestamp_us),
            channel_id: ChannelId(ChannelId::REAL_TIME.0),
            feeds: vec![PayloadFeedData {
                feed_id: PriceFeedId(FEED_ID),
                properties: full_props(price, ema, conf, timestamp_us),
            }],
        },
    )
}

/// Build a single-feed (FEED_ID) signed payload with arbitrary properties.
fn build_payload(timestamp_us: u64, channel: u8, properties: Vec<PayloadPropertyValue>) -> Vec<u8> {
    sign(&PayloadData {
        timestamp_us: TimestampUs::from_micros(timestamp_us),
        channel_id: ChannelId(channel),
        feeds: vec![PayloadFeedData {
            feed_id: PriceFeedId(FEED_ID),
            properties,
        }],
    })
}

/// The standard property set: spot price + confidence + exponent + EMA price + per-feed timestamp.
fn full_props(price: i64, ema: i64, conf: i64, timestamp_us: u64) -> Vec<PayloadPropertyValue> {
    vec![
        PayloadPropertyValue::Price(Some(Price::from_mantissa(price).unwrap())),
        PayloadPropertyValue::Confidence(Some(Price::from_mantissa(conf).unwrap())),
        PayloadPropertyValue::Exponent(EXPO),
        PayloadPropertyValue::EmaPrice(Some(Price::from_mantissa(ema).unwrap())),
        PayloadPropertyValue::EmaConfidence(Some(Price::from_mantissa(conf).unwrap())),
        PayloadPropertyValue::FeedUpdateTimestamp(Some(TimestampUs::from_micros(timestamp_us))),
    ]
}

/// A well-formed single-feed payload carrying spot, EMA, confidence and a per-feed timestamp.
fn signed_payload(timestamp_us: u64, channel: u8, price: i64, ema: i64, conf: i64) -> Vec<u8> {
    build_payload(
        timestamp_us,
        channel,
        full_props(price, ema, conf, timestamp_us),
    )
}

fn real_time(timestamp_us: u64, price: i64, ema: i64, conf: i64) -> Vec<u8> {
    signed_payload(timestamp_us, ChannelId::REAL_TIME.0, price, ema, conf)
}

fn owner() -> AccountId {
    "owner.near".parse().unwrap()
}

fn config() -> ConfigArgs {
    ConfigArgs {
        signers: vec![TrustedSigner {
            public_key: signer_public_key(),
            expires_at_s: NOW_S + 1_000_000,
        }],
        max_timestamp_delay_s: 600,
        max_timestamp_ahead_s: 600,
        allowed_channel_id: Some(ChannelId::REAL_TIME.0),
        update_fee: NearToken::from_yoctonear(0),
        default_valid_time_period_s: 600,
    }
}

fn price_id() -> PriceIdentifier {
    PriceIdentifier([0x11; 32])
}

/// Context as the owner, with 1 yocto attached (for `#[payable]` admin methods).
fn set_owner_context() {
    testing_env!(VMContextBuilder::new()
        .predecessor_account_id(owner())
        .attached_deposit(NearToken::from_yoctonear(1))
        .block_timestamp(NOW_S * 1_000_000_000)
        .build());
}

/// Context as an arbitrary relayer attaching `deposit` (for the permissionless update + views).
fn relayer_context(deposit: NearToken) {
    testing_env!(VMContextBuilder::new()
        .predecessor_account_id("relayer.near".parse().unwrap())
        .attached_deposit(deposit)
        .block_timestamp(NOW_S * 1_000_000_000)
        .build());
}

/// Enough to cover one feed's storage in any test.
fn ample_deposit() -> NearToken {
    NearToken::from_near(1)
}

fn deploy_and_map() -> Contract {
    deploy_and_map_with(config())
}

fn deploy_and_map_with(config: ConfigArgs) -> Contract {
    set_owner_context();
    let mut contract = Contract::new(owner(), config);
    contract.admin_set_feed_mapping(price_id(), Some(FEED_ID));
    contract
}

#[test]
fn ingests_payload_and_serves_pyth_views() {
    let mut contract = deploy_and_map();

    relayer_context(ample_deposit());
    contract.update_price_feeds(Base64VecU8(real_time(
        NOW_S * 1_000_000,
        123_456,
        123_000,
        50,
    )));

    assert!(contract.price_feed_exists(price_id()));

    let ema = contract.list_ema_prices_no_older_than(vec![price_id()], 600);
    let price = ema.get(&price_id()).unwrap().as_ref().unwrap();
    assert_eq!(price.price.0, 123_000);
    assert_eq!(price.conf.0, 50);
    assert_eq!(price.expo, -8);
    assert_eq!(price.publish_time.as_secs(), i64::try_from(NOW_S).unwrap());

    let spot = contract.get_price_unsafe(price_id()).unwrap();
    assert_eq!(spot.price.0, 123_456);

    // Unmapped identifier resolves to nothing.
    let unknown = PriceIdentifier([0x22; 32]);
    assert!(!contract.price_feed_exists(unknown));
    assert!(contract
        .list_ema_prices_no_older_than(vec![unknown], 600)
        .get(&unknown)
        .unwrap()
        .is_none());
}

#[test]
fn non_suffixed_views_serve_fresh_data() {
    let mut contract = deploy_and_map();

    relayer_context(ample_deposit());
    contract.update_price_feeds(Base64VecU8(real_time(
        NOW_S * 1_000_000,
        123_456,
        123_000,
        50,
    )));

    // The non-suffixed Pyth methods serve data within the default validity window.
    assert_eq!(contract.get_price(price_id()).unwrap().price.0, 123_456);
    assert_eq!(contract.get_ema_price(price_id()).unwrap().price.0, 123_000);
    assert!(contract
        .list_prices(vec![price_id()])
        .get(&price_id())
        .unwrap()
        .is_some());
    assert!(contract
        .list_ema_prices(vec![price_id()])
        .get(&price_id())
        .unwrap()
        .is_some());
}

#[test]
fn non_suffixed_views_apply_default_validity() {
    // Default validity 100s, but the ingestion window stays at 600s.
    let mut contract = deploy_and_map_with(ConfigArgs {
        default_valid_time_period_s: 100,
        ..config()
    });

    relayer_context(ample_deposit());
    // Published 500s ago: stored (within the 600s ingestion window)...
    contract.update_price_feeds(Base64VecU8(real_time(
        (NOW_S - 500) * 1_000_000,
        123_456,
        123_000,
        50,
    )));

    // ...but older than the 100s default window, so the non-suffixed views report nothing.
    assert!(contract.get_price(price_id()).is_none());
    assert!(contract.get_ema_price(price_id()).is_none());
    assert!(contract
        .list_prices(vec![price_id()])
        .get(&price_id())
        .unwrap()
        .is_none());
    assert!(contract
        .list_ema_prices(vec![price_id()])
        .get(&price_id())
        .unwrap()
        .is_none());

    // The unsafe + explicit-window variants still serve it.
    assert!(contract.get_price_unsafe(price_id()).is_some());
    assert_eq!(
        contract
            .get_price_no_older_than(price_id(), 600)
            .unwrap()
            .price
            .0,
        123_456
    );
}

#[test]
fn stale_prices_are_filtered_by_age() {
    let mut contract = deploy_and_map();

    relayer_context(ample_deposit());
    // Published 500s ago; within the 600s verification window so it stores...
    contract.update_price_feeds(Base64VecU8(real_time(
        (NOW_S - 500) * 1_000_000,
        123_456,
        123_000,
        50,
    )));

    // ...but a 100s freshness query rejects it, while unsafe + a wide window accept it.
    assert!(contract
        .list_ema_prices_no_older_than(vec![price_id()], 100)
        .get(&price_id())
        .unwrap()
        .is_none());
    assert!(contract
        .list_ema_prices_no_older_than(vec![price_id()], 600)
        .get(&price_id())
        .unwrap()
        .is_some());
    assert!(contract
        .list_ema_prices_unsafe(vec![price_id()])
        .get(&price_id())
        .unwrap()
        .is_some());
}

#[test]
fn replays_are_ignored_and_newer_updates_apply() {
    let mut contract = deploy_and_map();

    relayer_context(ample_deposit());
    let first = real_time(NOW_S * 1_000_000, 100, 100, 1);
    contract.update_price_feeds(Base64VecU8(first.clone()));

    // Same timestamp again => ignored (monotonic gate); price stays 100. Overwrite attempt consumes
    // no storage, so a zero deposit is accepted.
    relayer_context(NearToken::from_yoctonear(0));
    contract.update_price_feeds(Base64VecU8(first));
    assert_eq!(contract.get_price_unsafe(price_id()).unwrap().price.0, 100);

    // A strictly newer payload applies (same footprint => still free).
    contract.update_price_feeds(Base64VecU8(real_time((NOW_S + 1) * 1_000_000, 200, 200, 1)));
    assert_eq!(contract.get_price_unsafe(price_id()).unwrap().price.0, 200);
}

#[test]
fn full_deposit_refunded_when_no_new_storage() {
    let mut contract = deploy_and_map();

    // First update creates the feed (consumes storage).
    relayer_context(ample_deposit());
    contract.update_price_feeds(Base64VecU8(real_time(NOW_S * 1_000_000, 100, 100, 1)));

    // A strictly-newer overwrite consumes no new storage; with update_fee = 0 the whole attached
    // deposit must be refunded. (relayer_context resets the VM, so only this call's receipts remain.)
    let deposit = NearToken::from_near(1);
    relayer_context(deposit);
    contract.update_price_feeds(Base64VecU8(real_time((NOW_S + 1) * 1_000_000, 200, 200, 1)));

    let refunds: Vec<NearToken> = get_created_receipts()
        .into_iter()
        .flat_map(|receipt| receipt.actions)
        .filter_map(|action| match action {
            MockAction::Transfer { deposit, .. } => Some(deposit),
            _ => None,
        })
        .collect();
    assert_eq!(refunds, vec![deposit]);
}

#[test]
fn newer_package_with_older_feed_timestamp_does_not_regress() {
    let mut contract = deploy_and_map();

    relayer_context(ample_deposit());
    // First: package + per-feed timestamp at NOW.
    contract.update_price_feeds(Base64VecU8(build_payload(
        NOW_S * 1_000_000,
        ChannelId::REAL_TIME.0,
        full_props(100, 100, 1, NOW_S * 1_000_000),
    )));

    // Second: newer *package* timestamp (NOW+10) but an *older* per-feed FeedUpdateTimestamp
    // (NOW-10). The per-feed monotonic gate must reject it, so the stored feed does not regress.
    relayer_context(NearToken::from_yoctonear(0));
    contract.update_price_feeds(Base64VecU8(build_payload(
        (NOW_S + 10) * 1_000_000,
        ChannelId::REAL_TIME.0,
        full_props(999, 999, 1, (NOW_S - 10) * 1_000_000),
    )));

    let feed = contract.get_price_unsafe(price_id()).unwrap();
    assert_eq!(feed.price.0, 100);
    assert_eq!(feed.publish_time.as_secs(), i64::try_from(NOW_S).unwrap());
}

#[test]
fn future_feed_timestamp_is_rejected() {
    let mut contract = deploy_and_map();

    relayer_context(ample_deposit());
    // Package timestamp is current (passes the verifier window), but the per-feed FeedUpdateTimestamp
    // is far in the future (beyond max_timestamp_ahead_s), so the feed must not be stored.
    contract.update_price_feeds(Base64VecU8(build_payload(
        NOW_S * 1_000_000,
        ChannelId::REAL_TIME.0,
        full_props(123_456, 123_000, 50, (NOW_S + 10_000) * 1_000_000),
    )));

    assert!(!contract.price_feed_exists(price_id()));
}

#[test]
fn future_feed_within_tolerance_is_stored_but_not_fresh() {
    let mut contract = deploy_and_map();

    relayer_context(ample_deposit());
    // Per-feed timestamp 5s ahead of block time: within max_timestamp_ahead_s, so it is stored...
    contract.update_price_feeds(Base64VecU8(build_payload(
        NOW_S * 1_000_000,
        ChannelId::REAL_TIME.0,
        full_props(123_456, 123_000, 50, (NOW_S + 5) * 1_000_000),
    )));
    assert!(contract.price_feed_exists(price_id()));

    // ...but a future publish time is never "fresh": the age-gated and non-suffixed views fail
    // closed (matching the proxy-oracle cache).
    assert!(contract.get_price_no_older_than(price_id(), 600).is_none());
    assert!(contract
        .get_ema_price_no_older_than(price_id(), 600)
        .is_none());
    assert!(contract.get_price(price_id()).is_none());
    assert!(contract.get_ema_price(price_id()).is_none());

    // The unsafe variants make no freshness promise, so they still expose it.
    assert_eq!(
        contract.get_price_unsafe(price_id()).unwrap().price.0,
        123_456
    );
}

#[test]
fn missing_spot_confidence_skips_feed() {
    let mut contract = deploy_and_map();

    relayer_context(NearToken::from_yoctonear(0));
    // Price present but no Confidence property: not definitely-correct, so the feed is not stored.
    contract.update_price_feeds(Base64VecU8(build_payload(
        NOW_S * 1_000_000,
        ChannelId::REAL_TIME.0,
        vec![
            PayloadPropertyValue::Price(Some(Price::from_mantissa(123_456).unwrap())),
            PayloadPropertyValue::Exponent(EXPO),
            PayloadPropertyValue::EmaPrice(Some(Price::from_mantissa(123_000).unwrap())),
            PayloadPropertyValue::EmaConfidence(Some(Price::from_mantissa(50).unwrap())),
            PayloadPropertyValue::FeedUpdateTimestamp(Some(TimestampUs::from_micros(
                NOW_S * 1_000_000,
            ))),
        ],
    )));
    assert!(!contract.price_feed_exists(price_id()));
}

#[test]
fn ema_price_without_ema_confidence_skips_feed() {
    let mut contract = deploy_and_map();

    relayer_context(NearToken::from_yoctonear(0));
    // EmaPrice present but no EmaConfidence: a half-specified EMA is malformed, so the whole feed
    // is skipped (no spot-only storage, no fabricated confidence).
    contract.update_price_feeds(Base64VecU8(build_payload(
        NOW_S * 1_000_000,
        ChannelId::REAL_TIME.0,
        vec![
            PayloadPropertyValue::Price(Some(Price::from_mantissa(123_456).unwrap())),
            PayloadPropertyValue::Confidence(Some(Price::from_mantissa(50).unwrap())),
            PayloadPropertyValue::Exponent(EXPO),
            PayloadPropertyValue::EmaPrice(Some(Price::from_mantissa(123_000).unwrap())),
            PayloadPropertyValue::FeedUpdateTimestamp(Some(TimestampUs::from_micros(
                NOW_S * 1_000_000,
            ))),
        ],
    )));
    assert!(!contract.price_feed_exists(price_id()));
}

#[test]
fn duplicate_feed_id_in_payload_is_first_wins() {
    let mut contract = deploy_and_map();

    let feed = |price: i64| PayloadFeedData {
        feed_id: PriceFeedId(FEED_ID),
        properties: vec![
            PayloadPropertyValue::Price(Some(Price::from_mantissa(price).unwrap())),
            PayloadPropertyValue::Confidence(Some(Price::from_mantissa(1).unwrap())),
            PayloadPropertyValue::Exponent(EXPO),
            PayloadPropertyValue::FeedUpdateTimestamp(Some(TimestampUs::from_micros(
                NOW_S * 1_000_000,
            ))),
        ],
    };
    let payload = sign(&PayloadData {
        timestamp_us: TimestampUs::from_micros(NOW_S * 1_000_000),
        channel_id: ChannelId::REAL_TIME,
        feeds: vec![feed(111), feed(222)],
    });

    relayer_context(ample_deposit());
    contract.update_price_feeds(Base64VecU8(payload));

    // Both entries carry the same publish timestamp, so the monotonic gate keeps the first.
    assert_eq!(contract.get_price_unsafe(price_id()).unwrap().price.0, 111);
}

#[test]
fn negative_confidence_skips_feed() {
    let mut contract = deploy_and_map();

    // Negative spot confidence is malformed -> the feed is skipped entirely (no storage consumed).
    relayer_context(NearToken::from_yoctonear(0));
    contract.update_price_feeds(Base64VecU8(real_time(
        NOW_S * 1_000_000,
        123_456,
        123_000,
        -1,
    )));
    assert!(!contract.price_feed_exists(price_id()));

    // Negative EMA confidence likewise skips the feed.
    contract.update_price_feeds(Base64VecU8(build_payload(
        NOW_S * 1_000_000,
        ChannelId::REAL_TIME.0,
        vec![
            PayloadPropertyValue::Price(Some(Price::from_mantissa(123_456).unwrap())),
            PayloadPropertyValue::Confidence(Some(Price::from_mantissa(50).unwrap())),
            PayloadPropertyValue::Exponent(EXPO),
            PayloadPropertyValue::EmaPrice(Some(Price::from_mantissa(123_000).unwrap())),
            PayloadPropertyValue::EmaConfidence(Some(Price::from_mantissa(-1).unwrap())),
            PayloadPropertyValue::FeedUpdateTimestamp(Some(TimestampUs::from_micros(
                NOW_S * 1_000_000,
            ))),
        ],
    )));
    assert!(!contract.price_feed_exists(price_id()));
}

#[test]
fn payload_without_ema_serves_spot_only() {
    let mut contract = deploy_and_map();

    relayer_context(ample_deposit());
    // No EmaPrice property: spot is stored, but EMA must not be synthesized from spot.
    contract.update_price_feeds(Base64VecU8(build_payload(
        NOW_S * 1_000_000,
        ChannelId::REAL_TIME.0,
        vec![
            PayloadPropertyValue::Price(Some(Price::from_mantissa(123_456).unwrap())),
            PayloadPropertyValue::Confidence(Some(Price::from_mantissa(50).unwrap())),
            PayloadPropertyValue::Exponent(EXPO),
            PayloadPropertyValue::FeedUpdateTimestamp(Some(TimestampUs::from_micros(
                NOW_S * 1_000_000,
            ))),
        ],
    )));

    // Spot works...
    assert_eq!(
        contract.get_price_unsafe(price_id()).unwrap().price.0,
        123_456
    );
    // ...but the EMA surface returns nothing (no spot fallback).
    assert!(contract.get_ema_price_unsafe(price_id()).is_none());
    assert!(contract
        .list_ema_prices_unsafe(vec![price_id()])
        .get(&price_id())
        .unwrap()
        .is_none());
}

#[test]
#[should_panic(expected = "Insufficient deposit")]
fn insufficient_storage_deposit_for_new_feed_panics() {
    let mut contract = deploy_and_map();

    // A brand-new feed consumes storage; a zero deposit cannot cover it.
    relayer_context(NearToken::from_yoctonear(0));
    contract.update_price_feeds(Base64VecU8(real_time(
        NOW_S * 1_000_000,
        123_456,
        123_000,
        50,
    )));
}

#[test]
#[should_panic(expected = "Insufficient deposit")]
fn update_fee_must_be_covered() {
    let mut contract = deploy_and_map_with(ConfigArgs {
        update_fee: NearToken::from_near(5),
        ..config()
    });

    // The deposit covers storage but not the 5 NEAR fee.
    relayer_context(NearToken::from_millinear(100));
    contract.update_price_feeds(Base64VecU8(real_time(
        NOW_S * 1_000_000,
        123_456,
        123_000,
        50,
    )));
}

#[test]
fn update_fee_is_retained_and_excess_refunded() {
    let update_fee = NearToken::from_millinear(10);
    let mut contract = deploy_and_map_with(ConfigArgs {
        update_fee,
        ..config()
    });

    // First update creates the feed (consumes storage); fund it amply.
    relayer_context(ample_deposit());
    contract.update_price_feeds(Base64VecU8(real_time(NOW_S * 1_000_000, 100, 100, 1)));
    assert!(contract.price_feed_exists(price_id()));

    // A strictly-newer overwrite consumes no new storage, so the only charge is `update_fee`: the
    // refund must be exactly deposit - update_fee. (relayer_context resets the VM, so only this
    // call's receipts remain; no-new-storage zeroes the storage term so the arithmetic is exact.)
    let deposit = NearToken::from_near(1);
    relayer_context(deposit);
    contract.update_price_feeds(Base64VecU8(real_time((NOW_S + 1) * 1_000_000, 200, 200, 1)));

    let refunds: Vec<NearToken> = get_created_receipts()
        .into_iter()
        .flat_map(|receipt| receipt.actions)
        .filter_map(|action| match action {
            MockAction::Transfer { deposit, .. } => Some(deposit),
            _ => None,
        })
        .collect();
    assert_eq!(refunds, vec![deposit.saturating_sub(update_fee)]);
}

#[test]
#[should_panic(expected = "signer is not trusted")]
fn rejects_untrusted_signer_payload() {
    let mut contract = deploy_and_map();

    // Reconfigure to trust a different signer, then submit a payload from the original key.
    set_owner_context();
    let mut cfg = config();
    cfg.signers[0].public_key = [0xAB; 32];
    contract.admin_set_config(cfg);

    relayer_context(ample_deposit());
    contract.update_price_feeds(Base64VecU8(real_time(NOW_S * 1_000_000, 100, 100, 1)));
}

#[test]
#[should_panic(expected = "Owner only")]
fn admin_methods_reject_non_owner() {
    let mut contract = deploy_and_map();

    // Non-owner attempting an admin mutation must panic (1 yocto attached to pass the payable gate).
    testing_env!(VMContextBuilder::new()
        .predecessor_account_id("attacker.near".parse().unwrap())
        .attached_deposit(NearToken::from_yoctonear(1))
        .block_timestamp(NOW_S * 1_000_000_000)
        .build());
    contract.admin_set_feed_mapping(PriceIdentifier([0x33; 32]), Some(99));
}

#[test]
#[should_panic(expected = "Owner only")]
fn admin_withdraw_rejects_non_owner() {
    let mut contract = deploy_and_map();

    testing_env!(VMContextBuilder::new()
        .predecessor_account_id("attacker.near".parse().unwrap())
        .attached_deposit(NearToken::from_yoctonear(1))
        .block_timestamp(NOW_S * 1_000_000_000)
        .build());
    let _ = contract.admin_withdraw(NearToken::from_yoctonear(1));
}

#[test]
fn owner_can_withdraw() {
    let mut contract = deploy_and_map();
    set_owner_context();
    // Owner withdrawal schedules a transfer without panicking.
    let _ = contract.admin_withdraw(NearToken::from_yoctonear(1));
}

// --- Config validation (W1) ---

#[test]
#[should_panic(expected = "signer set must not be empty")]
fn empty_signer_set_rejected() {
    set_owner_context();
    let _ = Contract::new(
        owner(),
        ConfigArgs {
            signers: vec![],
            ..config()
        },
    );
}

#[test]
#[should_panic(expected = "duplicate signer public key")]
fn duplicate_signers_rejected() {
    set_owner_context();
    let signer = TrustedSigner {
        public_key: signer_public_key(),
        expires_at_s: NOW_S + 1,
    };
    let _ = Contract::new(
        owner(),
        ConfigArgs {
            signers: vec![signer.clone(), signer],
            ..config()
        },
    );
}

#[test]
fn signer_set_serializes_in_public_key_order() {
    // The `BTreeMap`-backed `SignerSet` yields a deterministic, public-key-sorted array regardless
    // of insertion order — locking the structural-uniqueness representation's wire behavior.
    let mut contract = deploy_and_map();
    set_owner_context();
    // Insert two signers that bracket the default key, in non-sorted order.
    contract.admin_set_signer(hex::encode([0xFF; 32]), Some(NOW_S + 1));
    contract.admin_set_signer(hex::encode([0x00; 32]), Some(NOW_S + 1));

    let json = near_sdk::serde_json::to_value(&contract.get_config().signers).unwrap();
    let keys: Vec<String> = json
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["public_key"].as_str().unwrap().to_string())
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "signers must serialize in public-key order");
    assert_eq!(keys.len(), 3);
}

#[test]
#[should_panic(expected = "max_timestamp_delay_s must be non-zero")]
fn zero_delay_window_rejected() {
    set_owner_context();
    let _ = Contract::new(
        owner(),
        ConfigArgs {
            max_timestamp_delay_s: 0,
            ..config()
        },
    );
}

#[test]
#[should_panic(expected = "default_valid_time_period_s must be non-zero")]
fn zero_default_validity_rejected() {
    set_owner_context();
    let _ = Contract::new(
        owner(),
        ConfigArgs {
            default_valid_time_period_s: 0,
            ..config()
        },
    );
}

#[test]
#[should_panic(expected = "signer set must not be empty")]
fn admin_set_config_validates() {
    let mut contract = deploy_and_map();
    set_owner_context();
    contract.admin_set_config(ConfigArgs {
        signers: vec![],
        ..config()
    });
}

#[test]
#[should_panic(expected = "cannot remove the last signer")]
fn admin_set_signer_cannot_remove_last() {
    let mut contract = deploy_and_map();
    set_owner_context();
    // config() has exactly one signer; removing it (`None`) must be rejected.
    contract.admin_set_signer(hex::encode(signer_public_key()), None);
}

#[test]
fn admin_set_signer_removes_non_last_signer() {
    let mut contract = deploy_and_map();
    set_owner_context();

    // Add a second signer, then remove the original — leaving exactly the new one.
    let other = [0xCD; 32];
    contract.admin_set_signer(hex::encode(other), Some(NOW_S + 1));
    contract.admin_set_signer(hex::encode(signer_public_key()), None);

    let json = near_sdk::serde_json::to_value(&contract.get_config().signers).unwrap();
    let keys: Vec<String> = json
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["public_key"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(keys, vec![hex::encode(other)]);
}

// --- Signer rotation (Solana key rotation) ---

#[test]
fn rotating_in_a_new_signer_accepts_its_updates() {
    // A second Lazer key — the rotation target. (Pyth rotates its ed25519 signer on the Solana
    // program; the owner mirrors that here via `admin_set_signer`.)
    let new_key = SigningKey::from_bytes(&[9u8; 32]);

    let mut contract = deploy_and_map();

    // Owner rotates the new key in (keeping the existing one during the overlap window).
    set_owner_context();
    contract.admin_set_signer(
        hex::encode(new_key.verifying_key().to_bytes()),
        Some(NOW_S + 1_000_000),
    );

    // A payload signed by the newly trusted key now verifies and stores.
    relayer_context(ample_deposit());
    contract.update_price_feeds(Base64VecU8(real_time_signed_by(
        &new_key,
        NOW_S * 1_000_000,
        123_456,
        123_000,
        50,
    )));
    assert_eq!(
        contract.get_price_unsafe(price_id()).unwrap().price.0,
        123_456
    );
}

#[test]
#[should_panic(expected = "signer is not trusted")]
fn refreshing_signer_expiry_into_the_past_rejects_updates() {
    let mut contract = deploy_and_map();

    // Refresh the (only) signer's expiry to `now` — i.e. lapse it (`expires_at_s > now` is false).
    // This exercises both upsert-refresh and the expiry gate in verification.
    set_owner_context();
    contract.admin_set_signer(hex::encode(signer_public_key()), Some(NOW_S));

    relayer_context(ample_deposit());
    contract.update_price_feeds(Base64VecU8(real_time(
        NOW_S * 1_000_000,
        123_456,
        123_000,
        50,
    )));
}

// --- Stateless verify_update view ---

#[test]
fn verify_update_returns_data_without_writing_storage() {
    let contract = deploy_and_map();

    relayer_context(NearToken::from_yoctonear(0)); // a view: no deposit needed
    let view = contract.verify_update(Base64VecU8(real_time(
        NOW_S * 1_000_000,
        123_456,
        123_000,
        50,
    )));

    // Full verified update is returned.
    assert_eq!(view.signer, signer_public_key());
    assert_eq!(view.timestamp_ns.as_secs(), NOW_S);
    assert_eq!(view.feeds.len(), 1);
    let feed = &view.feeds[0];
    assert_eq!(feed.feed_id, FEED_ID);
    assert_eq!(feed.price.unwrap().0, 123_456);
    assert_eq!(feed.ema_price.unwrap().0, 123_000);
    assert_eq!(feed.confidence.unwrap().0, 50);
    assert_eq!(feed.exponent, Some(EXPO));

    // ...and nothing was persisted.
    assert!(!contract.price_feed_exists(price_id()));
    assert!(contract.get_feed_data(FEED_ID).is_none());
}

#[test]
fn verify_update_surfaces_non_pyth_properties() {
    let contract = deploy_and_map();
    relayer_context(NearToken::from_yoctonear(0));

    // A payload carrying properties outside the Pyth subset.
    let view = contract.verify_update(Base64VecU8(build_payload(
        NOW_S * 1_000_000,
        ChannelId::REAL_TIME.0,
        vec![
            PayloadPropertyValue::Price(Some(Price::from_mantissa(100).unwrap())),
            PayloadPropertyValue::Exponent(EXPO),
            PayloadPropertyValue::PublisherCount(9),
            PayloadPropertyValue::BestBidPrice(Some(Price::from_mantissa(99).unwrap())),
        ],
    )));
    let feed = &view.feeds[0];
    assert_eq!(feed.publisher_count, Some(9));
    assert_eq!(feed.best_bid_price.unwrap().0, 99);
}

#[test]
#[should_panic(expected = "signer is not trusted")]
fn verify_update_rejects_untrusted_signer() {
    let key_b = SigningKey::from_bytes(&[9u8; 32]);
    let contract = deploy_and_map(); // trusts the default key, not key_b

    relayer_context(NearToken::from_yoctonear(0));
    let _ = contract.verify_update(Base64VecU8(real_time_signed_by(
        &key_b,
        NOW_S * 1_000_000,
        100,
        100,
        1,
    )));
}
