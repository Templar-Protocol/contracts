use std::{
    collections::HashMap,
    panic::{catch_unwind, AssertUnwindSafe},
};

use hex_literal::hex;
use near_sdk::{
    test_utils::{self, VMContextBuilder},
    testing_env, AccountId, NearToken,
};
use primitive_types::U256;
use templar_common::oracle::redstone::{
    config, Config, FeedData, FeedId, RedStoneContractInterface, Role,
};
use templar_primitives::time::Nanoseconds;

use templar_redstone_adapter_contract::*;

const FIXTURE_TIMESTAMP_MS: u64 = 1_770_985_144_000;
const LATER_FIXTURE_TIMESTAMP_MS: u64 = 1_771_336_150_000;
const ETH_BTC: &str = include_str!("fixtures/eth_btc.hex");
const ETH_BTC_LATER: &str = include_str!("fixtures/eth_btc_later.hex");

#[allow(
    clippy::unwrap_used,
    reason = "checked-in hex fixtures must decode or fail the test"
)]
fn signed_payload(fixture: &str) -> Vec<u8> {
    fixture
        .trim()
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[allow(
    clippy::unwrap_used,
    reason = "invalid test account literals must fail immediately"
)]
fn context(predecessor: &str, timestamp_ms: u64, deposit: u128) {
    testing_env!(VMContextBuilder::new()
        .current_account_id("adapter.near".parse().unwrap())
        .predecessor_account_id(predecessor.parse().unwrap())
        .block_timestamp(timestamp_ms * 1_000_000)
        .attached_deposit(NearToken::from_yoctonear(deposit))
        .build());
}

#[allow(
    clippy::unwrap_used,
    reason = "the test administrator is a fixed valid account literal"
)]
fn new_adapter() -> Contract {
    context("registry.near", FIXTURE_TIMESTAMP_MS, 0);
    Contract::new(config::prod(), "owner.near".parse().unwrap())
}

fn set_config(adapter: &mut Contract, config: Config) {
    context("owner.near", FIXTURE_TIMESTAMP_MS, 1);
    adapter.admin_set_config(config);
}

#[allow(
    clippy::expect_used,
    reason = "asserting the rejected call's panic and diagnostic"
)]
fn assert_rejected(action: impl FnOnce(), expected: &str) {
    let panic = catch_unwind(AssertUnwindSafe(action)).expect_err("call must reject");
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .expect("panic must have a message");
    assert!(message.contains(expected), "unexpected panic: {message}");
}

fn role_members(adapter: &Contract) -> (Vec<AccountId>, Vec<AccountId>) {
    (
        adapter.list_role(Role::Admin, None, None),
        adapter.list_role(Role::TrustedUpdater, None, None),
    )
}

/// The specified administrator receives both roles, not the registry predecessor.
#[test]
fn new_seeds_roles_on_explicit_admin() {
    testing_env!(VMContextBuilder::new()
        .current_account_id("adapter.near".parse().unwrap())
        .predecessor_account_id("registry.near".parse().unwrap())
        .build());

    let adapter = Contract::new(config::prod(), "operator.near".parse().unwrap());
    let admin = "operator.near".parse::<near_sdk::AccountId>().unwrap();
    let predecessor = "registry.near".parse::<near_sdk::AccountId>().unwrap();

    assert!(adapter.has_role(admin.clone(), Role::Admin));
    assert!(adapter.has_role(admin, Role::TrustedUpdater));
    assert!(!adapter.has_role(predecessor.clone(), Role::Admin));
    assert!(!adapter.has_role(predecessor, Role::TrustedUpdater));
}

#[rstest::rstest]
#[case::stellar(FIXTURE_TIMESTAMP_MS, &signed_payload(ETH_BTC))]
#[case::js_sdk(LATER_FIXTURE_TIMESTAMP_MS, &signed_payload(ETH_BTC_LATER))]
#[should_panic = "called `Option::unwrap()` on a `None` value"]
#[case::stellar_feed_id(1_770_985_144_000, &hex!("A5544800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002d9030a710019c56f0bec0000000200000015d1cb1a708c63264741b00ce097176e45f708914b8cfdca26b079877a70604e25aa0bcfa3a41df8212eddd51db3496b95c7c3dc4caa9ac9705602af0515db1b31c45544800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002d9028ed04019c56f0bec000000020000001dcaf484941c0d206f1898185b953c6a92d7fd188b347505c0f5beb2030e06e3e1b2f7dfb45929ac7676136af93fee7f14a614b40fa4dc2d1e625dbece02eaca21c45544800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002d9028ed04019c56f0bec00000002000000199bd54930138268baad2869e9ceb99b6bc67cd6b8a4cc98e05f0b1cd9b7f07066008208399a728fac3d1dc3ca407cb8199a0209377bceb0c48f2cc3d756078051b4254430000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000006179a92ab8c019c56f0bec000000020000001f08af53ed34046f7f64cc02ffb7973252954d7c395e440693c896bffdbc2de1e31cf5675bf66583d3e3438f5002ae9c10870d4dc45de05c560b239aa3a2d50a41b425443000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000617a1187473019c56f0bec0000000200000011b96dc2763a692e3245ce4f1b0c16ea245c240204e99ebd323b340e58bfb14fb5f0465ce11b8dd52ff839547cc949d20e4e8ba0be43dd6417cade2a8ebfd8c9e1c425443000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000617a1187473019c56f0bec00000002000000114a02710892325b13afc74bbd350dd9ec80342b2d6c0c94df7b7a60dbf67a1b91b182fa4555e0e0db91e6258b279f00b7eeb8f5de9930e352d5321a6b8b64a031c00063137373039383531343539383223302e392e30237374656c6c61722d636f6e6e6563746f72000025000002ed57011e0000"))]
#[should_panic = "too old"]
#[case::stellar_timestamp_old(2_770_985_144_000, &signed_payload(ETH_BTC))]
#[should_panic = "too future"]
#[case::stellar_timestamp_future(1_270_985_144_000, &signed_payload(ETH_BTC))]
#[should_panic = "called `Option::unwrap()` on a `None` value"]
#[case::js_sdk_feed_id(1_771_336_150_000, &hex!("A2544300000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000062f31d92220019c6bdcabf000000020000001bcf952b4490f2e1d3eb14363fafb17785628ff35574c473a425c1b91bc7d69b32f5076efc05eeb87bd731cdd4573991e18b2e51d32cb6d338d72ec63b495b1be1c42544300000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000062f31d92220019c6bdcabf0000000200000012d94c6893f264b770f3af4e27f1053fef182f4fec1349e28ef0827b99a846409178e4d0629f89da7dacc2b923459d761629f7c2535f41419649eafa3f4b8f2901b42544300000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000062f2fbbb4d0019c6bdcabf000000020000001f4399f7127decb4c5f5d51e3fbdc70ea877f790cff4a262c3b8856ece72365273dcfcfe62aafda679c928c7aba4bed48e36e0d8d06f3eef2dea8c69696863aa01c45544800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002e22882520019c6bdcabf0000000200000018df63ce1bea5fc8738cb02cb09141604906fcba269e9fb1f873bc6a9edb705f07f963841bc56c4bd7c8b9748d243bc0113acfb59f6c6973b91a61550c260736f1b45544800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002e22882520019c6bdcabf0000000200000014060619a0ce0de8d714d0b0e0c9f86be2e2a8d841a63261079fa8e5bddc951b672aa9eeebf0780ff442ef3dc65530b1af3817eb8bc87ddd5bf44851702cfdc671c45544800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002e22882520019c6bdcabf0000000200000011c86e4b37fa2cd9a229594abe55056d8c55b7c1662cdbb4fff15881cf6e0a6043929c9b3965f63715de745cbcfd7292547609a015d5129352c5c1d699262fb241c0006000000000002ed57011e0000"))]
fn payload(#[case] timestamp: u64, #[case] input: &[u8]) {
    let context = VMContextBuilder::new()
        .block_timestamp(timestamp * 1_000_000)
        .build();
    testing_env!(context);

    let mut ra = Contract::new(config::prod(), "bob.near".parse().unwrap());

    let prices = vec!["ETH".into(), "BTC".into()];

    ra.write_prices(prices.clone(), input.to_vec().into());

    let price_data = ra.read_price_data(prices.clone());

    let eth = price_data.get(&"ETH".into()).unwrap();
    let btc = price_data.get(&"BTC".into()).unwrap();

    eprintln!("{price_data:#?}");

    assert_ne!(U256::from(eth.price), U256::zero());
    assert_ne!(U256::from(btc.price), U256::zero());
}

#[rstest::rstest]
fn output() {
    let timestamp = 1_770_985_144_000_u64;
    let input = signed_payload(ETH_BTC);

    let context = VMContextBuilder::new()
        .block_timestamp(timestamp * 1_000_000)
        .build();
    testing_env!(context);

    let mut ra = Contract::new(config::prod(), "bob.near".parse().unwrap());

    let prices = vec!["ETH".into(), "BTC".into()];

    ra.write_prices(prices.clone(), input.into());

    let price_data = ra.read_price_data(prices.clone());

    assert_eq!(
        price_data,
        HashMap::<FeedId, _>::from_iter([
            (
                "ETH".into(),
                FeedData {
                    price: U256::from(195_692_129_540_u128).into(),
                    package_timestamp: Nanoseconds::from_ms(1_770_985_144_000),
                    write_timestamp: Nanoseconds::from_ms(1_770_985_144_000),
                }
            ),
            (
                "BTC".into(),
                FeedData {
                    price: U256::from(6_698_556_748_915_u128).into(),
                    package_timestamp: Nanoseconds::from_ms(1_770_985_144_000),
                    write_timestamp: Nanoseconds::from_ms(1_770_985_144_000),
                }
            ),
        ])
    );

    assert_eq!(
        test_utils::get_logs()
            .into_iter()
            .filter(|log| log.starts_with(r#"EVENT_JSON:{"standard":"redstone-adapter""#))
            .collect::<Vec<_>>(),
        vec![
            r#"EVENT_JSON:{"standard":"redstone-adapter","version":"1.0.0","event":"write_prices","data":{"updater":"bob.near","updated_feeds":[["ETH",{"price":"195692129540","package_timestamp":"1770985144000000000","write_timestamp":"1770985144000000000"}],["BTC",{"price":"6698556748915","package_timestamp":"1770985144000000000","write_timestamp":"1770985144000000000"}]]}}"#,
        ],
    );
}

#[rstest::rstest]
#[case::stranger("stranger.near", 1, "Unauthorized role")]
#[case::trusted_updater("updater.near", 1, "Unauthorized role")]
#[case::no_deposit("owner.near", 0, "exactly 1 yoctoNEAR")]
#[case::excess_deposit("owner.near", 2, "exactly 1 yoctoNEAR")]
fn config_update_requires_admin_and_one_yocto(
    #[case] caller: &str,
    #[case] deposit: u128,
    #[case] expected: &str,
) {
    let mut adapter = new_adapter();
    context("owner.near", FIXTURE_TIMESTAMP_MS, 1);
    adapter.set_role("roles.near".parse().unwrap(), Role::Admin, None, None);
    adapter.set_role(
        "updater.near".parse().unwrap(),
        Role::TrustedUpdater,
        None,
        None,
    );
    let config = adapter.get_config().clone();
    let roles = role_members(&adapter);

    context(caller, FIXTURE_TIMESTAMP_MS, deposit);
    assert_rejected(
        || {
            adapter.admin_set_config(Config {
                max_timestamp_delay_ms: 0,
                ..config.clone()
            });
        },
        expected,
    );

    assert_eq!(adapter.get_config(), &config);
    assert_eq!(role_members(&adapter), roles);
}

#[test]
fn admin_without_updater_role_can_replace_configuration() {
    let mut adapter = new_adapter();
    context("owner.near", FIXTURE_TIMESTAMP_MS, 1);
    adapter.set_role("roles.near".parse().unwrap(), Role::Admin, None, None);
    adapter.set_role(
        "updater.near".parse().unwrap(),
        Role::TrustedUpdater,
        None,
        None,
    );
    context("roles.near", FIXTURE_TIMESTAMP_MS, 1);
    adapter.set_role(
        "owner.near".parse().unwrap(),
        Role::TrustedUpdater,
        Some(false),
        None,
    );
    let roles = role_members(&adapter);

    let config = Config {
        max_timestamp_delay_ms: 0,
        ..adapter.get_config().clone()
    };
    set_config(&mut adapter, config);

    assert_eq!(adapter.get_config().max_timestamp_delay_ms, 0);
    assert_eq!(role_members(&adapter), roles);
    assert!(adapter.has_role("owner.near".parse().unwrap(), Role::Admin));
    assert!(!adapter.has_role("owner.near".parse().unwrap(), Role::TrustedUpdater));
}

#[test]
fn later_snapshot_overwrites_every_prior_field() {
    let mut adapter = new_adapter();
    let original = adapter.get_config().clone();
    let replacement = Config {
        signer_count_threshold: 1,
        signers: vec![[1; 20]],
        max_timestamp_delay_ms: 0,
        max_timestamp_ahead_ms: u64::MAX,
        min_interval_between_updates_ms: 0,
    };
    set_config(&mut adapter, replacement.clone());
    assert_eq!(adapter.get_config(), &replacement);
    set_config(&mut adapter, original.clone());
    assert_eq!(adapter.get_config(), &original);
}

fn signer_addresses(count: u16) -> Vec<[u8; 20]> {
    (1..=count)
        .map(|index| {
            let mut address = [0; 20];
            address[..2].copy_from_slice(&index.to_le_bytes());
            address
        })
        .collect()
}

#[rstest::rstest]
#[case::one_signer(1)]
#[case::maximum_signers(255)]
fn all_five_fields_can_be_replaced_atomically(#[case] signer_count: u8) {
    let mut adapter = new_adapter();
    let expected = Config {
        signer_count_threshold: signer_count,
        signers: signer_addresses(u16::from(signer_count)),
        max_timestamp_delay_ms: u64::MAX,
        max_timestamp_ahead_ms: 0,
        min_interval_between_updates_ms: u64::MAX,
    };
    set_config(&mut adapter, expected.clone());
    assert_eq!(adapter.get_config(), &expected);
    assert_eq!(adapter.unique_signer_threshold().0, u64::from(signer_count));
}

#[rstest::rstest]
#[case::zero_quorum(
    0,
    signer_addresses(5),
    "signer_count_threshold must be greater than zero"
)]
#[case::empty_signers(1, vec![], "Wrong configuration signer count")]
#[case::excess_quorum(6, signer_addresses(5), "Wrong configuration signer count")]
#[case::duplicate_signers(1, vec![[1; 20]; 5], "is reoccurring")]
#[case::zero_address(1, vec![[0; 20]], "invalid signer")]
#[case::too_many_signers(1, signer_addresses(256), "allowed maximum")]
fn invalid_config_leaves_all_state_unchanged(
    #[case] threshold: u8,
    #[case] signers: Vec<[u8; 20]>,
    #[case] expected: &str,
) {
    let mut adapter = new_adapter();
    context("owner.near", FIXTURE_TIMESTAMP_MS, 0);
    adapter.write_prices(
        vec!["ETH".into(), "BTC".into()],
        signed_payload(ETH_BTC).into(),
    );
    context("owner.near", FIXTURE_TIMESTAMP_MS, 1);
    let config = adapter.get_config().clone();
    let feeds = adapter.read_price_data(vec!["ETH".into(), "BTC".into()]);
    let roles = role_members(&adapter);

    assert_rejected(
        || {
            adapter.admin_set_config(Config {
                signer_count_threshold: threshold,
                signers,
                max_timestamp_delay_ms: 0,
                ..config.clone()
            });
        },
        expected,
    );

    assert_eq!(adapter.get_config(), &config);
    assert_eq!(
        adapter.read_price_data(vec!["ETH".into(), "BTC".into()]),
        feeds
    );
    assert_eq!(role_members(&adapter), roles);
}

#[rstest::rstest]
#[case::past(false, "too old")]
#[case::future(true, "too future")]
fn tightening_timestamp_bounds_changes_payload_acceptance(
    #[case] future: bool,
    #[case] expected: &str,
) {
    let mut adapter = new_adapter();
    let now = if future {
        FIXTURE_TIMESTAMP_MS - 10_000
    } else {
        FIXTURE_TIMESTAMP_MS + 10_000
    };
    context("reader.near", now, 0);
    assert!(adapter
        .get_prices(
            vec!["ETH".into(), "BTC".into()],
            signed_payload(ETH_BTC).into()
        )
        .prices
        .contains_key(&"ETH".into()));

    context("owner.near", now, 1);
    let config = adapter.get_config().clone();
    adapter.admin_set_config(Config {
        max_timestamp_delay_ms: if future {
            config.max_timestamp_delay_ms
        } else {
            9_999
        },
        max_timestamp_ahead_ms: if future {
            9_999
        } else {
            config.max_timestamp_ahead_ms
        },
        ..config
    });
    context("reader.near", now, 0);
    assert_rejected(
        || {
            adapter.get_prices(
                vec!["ETH".into(), "BTC".into()],
                signed_payload(ETH_BTC).into(),
            );
        },
        expected,
    );
}

#[rstest::rstest]
#[case::untrusted(false)]
#[case::trusted(true)]
fn interval_replacement_preserves_trusted_path_and_package_replay_guard(#[case] trusted: bool) {
    let mut adapter = new_adapter();
    let config = Config {
        max_timestamp_delay_ms: u64::MAX,
        min_interval_between_updates_ms: 0,
        ..adapter.get_config().clone()
    };
    set_config(&mut adapter, config);
    let initial_write = LATER_FIXTURE_TIMESTAMP_MS - 1_000;
    context("writer.near", initial_write, 0);
    adapter.write_prices(
        vec!["ETH".into(), "BTC".into()],
        signed_payload(ETH_BTC).into(),
    );
    let initial = adapter.read_price_data(vec!["ETH".into(), "BTC".into()]);
    assert_eq!(initial.len(), 2);

    context("owner.near", LATER_FIXTURE_TIMESTAMP_MS, 1);
    adapter.admin_set_config(Config {
        min_interval_between_updates_ms: 60_000,
        ..adapter.get_config().clone()
    });
    let writer = if trusted { "owner.near" } else { "writer.near" };
    context(writer, LATER_FIXTURE_TIMESTAMP_MS, 0);
    adapter.write_prices(
        vec!["ETH".into(), "BTC".into()],
        signed_payload(ETH_BTC_LATER).into(),
    );
    if trusted {
        assert_eq!(
            adapter.read_timestamp("ETH".into()),
            Some(Nanoseconds::from_ms(LATER_FIXTURE_TIMESTAMP_MS))
        );
    } else {
        assert_eq!(
            adapter.read_price_data(vec!["ETH".into(), "BTC".into()]),
            initial
        );
    }

    context("owner.near", LATER_FIXTURE_TIMESTAMP_MS + 1, 1);
    adapter.admin_set_config(Config {
        min_interval_between_updates_ms: 0,
        ..adapter.get_config().clone()
    });
    context(writer, LATER_FIXTURE_TIMESTAMP_MS + 1, 0);
    adapter.write_prices(
        vec!["ETH".into(), "BTC".into()],
        signed_payload(ETH_BTC_LATER).into(),
    );
    let accepted = adapter.read_price_data(vec!["ETH".into(), "BTC".into()]);
    assert_eq!(accepted.len(), 2);
    assert_eq!(
        accepted[&FeedId::from("ETH")].package_timestamp,
        Nanoseconds::from_ms(LATER_FIXTURE_TIMESTAMP_MS)
    );
    assert_eq!(
        accepted[&FeedId::from("ETH")].write_timestamp,
        Nanoseconds::from_ms(LATER_FIXTURE_TIMESTAMP_MS + u64::from(!trusted))
    );

    context("owner.near", LATER_FIXTURE_TIMESTAMP_MS + 2, 1);
    adapter.admin_set_config(Config {
        max_timestamp_ahead_ms: 0,
        ..adapter.get_config().clone()
    });
    context(writer, LATER_FIXTURE_TIMESTAMP_MS + 2, 0);
    adapter.write_prices(
        vec!["ETH".into(), "BTC".into()],
        signed_payload(ETH_BTC_LATER).into(),
    );
    assert_eq!(
        adapter.read_price_data(vec!["ETH".into(), "BTC".into()]),
        accepted,
        "configuration changes must not reset package or write replay timestamps"
    );
}

#[test]
fn signer_rotation_changes_new_verification_without_revoking_cached_prices() {
    let mut adapter = new_adapter();
    context("owner.near", FIXTURE_TIMESTAMP_MS, 0);
    adapter.write_prices(
        vec!["ETH".into(), "BTC".into()],
        signed_payload(ETH_BTC).into(),
    );
    let cached = adapter.read_price_data(vec!["ETH".into(), "BTC".into()]);
    assert_eq!(
        cached[&FeedId::from("ETH")],
        FeedData {
            price: U256::from(195_692_129_540_u128).into(),
            package_timestamp: Nanoseconds::from_ms(FIXTURE_TIMESTAMP_MS),
            write_timestamp: Nanoseconds::from_ms(FIXTURE_TIMESTAMP_MS),
        }
    );
    let roles = role_members(&adapter);

    let config = Config {
        signers: vec![[1; 20]],
        signer_count_threshold: 1,
        ..adapter.get_config().clone()
    };
    set_config(&mut adapter, config);
    context("owner.near", FIXTURE_TIMESTAMP_MS + 1, 0);
    let verified = adapter.get_prices(
        vec!["ETH".into(), "BTC".into()],
        signed_payload(ETH_BTC).into(),
    );
    assert!(!verified.prices.contains_key(&"ETH".into()));
    adapter.write_prices(
        vec!["ETH".into(), "BTC".into()],
        signed_payload(ETH_BTC).into(),
    );
    assert_eq!(
        adapter.read_price_data(vec!["ETH".into(), "BTC".into()]),
        cached
    );
    assert_eq!(
        adapter.read_prices(vec!["ETH".into()])[&FeedId::from("ETH")],
        cached[&FeedId::from("ETH")].price
    );
    assert_eq!(role_members(&adapter), roles);
}

#[rstest::rstest]
#[case::stranger("stranger.near", 1, "Unauthorized role")]
#[case::updater("updater.near", 1, "Unauthorized role")]
#[case::zero_deposit("owner.near", 0, "exactly 1 yoctoNEAR")]
#[case::excess_deposit("owner.near", 2, "exactly 1 yoctoNEAR")]
fn role_updates_require_admin_and_one_yocto(
    #[case] caller: &str,
    #[case] deposit: u128,
    #[case] expected: &str,
) {
    let mut adapter = new_adapter();
    context("owner.near", FIXTURE_TIMESTAMP_MS, 1);
    adapter.set_role(
        "updater.near".parse().unwrap(),
        Role::TrustedUpdater,
        None,
        None,
    );
    let members = role_members(&adapter);
    context(caller, FIXTURE_TIMESTAMP_MS, deposit);
    assert_rejected(
        || adapter.set_role("stranger.near".parse().unwrap(), Role::Admin, None, None),
        expected,
    );
    assert_eq!(role_members(&adapter), members);
}

#[test]
fn role_defaults_and_last_admin_override_remain_explicit() {
    let mut adapter = new_adapter();
    context("owner.near", FIXTURE_TIMESTAMP_MS, 1);
    assert_rejected(
        || {
            adapter.set_role(
                "owner.near".parse().unwrap(),
                Role::Admin,
                Some(false),
                None,
            );
        },
        "Cannot remove the last member of a role",
    );
    adapter.set_role("roles.near".parse().unwrap(), Role::Admin, None, None);
    adapter.set_role(
        "owner.near".parse().unwrap(),
        Role::Admin,
        Some(false),
        None,
    );
    assert!(!adapter.has_role("owner.near".parse().unwrap(), Role::Admin));
    let config = adapter.get_config().clone();
    assert_rejected(
        || adapter.admin_set_config(config.clone()),
        "Unauthorized role",
    );
    context("roles.near", FIXTURE_TIMESTAMP_MS, 1);
    adapter.admin_set_config(config);
    adapter.set_role(
        "roles.near".parse().unwrap(),
        Role::Admin,
        Some(false),
        Some(true),
    );
    assert!(adapter.list_role(Role::Admin, None, None).is_empty());
    assert!(adapter.has_role("owner.near".parse().unwrap(), Role::TrustedUpdater));
}
