use std::{num::NonZero, path::Path, str::FromStr};

use crate::controller::vault::{UnifiedVaultController, VaultController};
pub use controller::{
    ft::FtController,
    market::{MarketController, UnifiedMarketController},
    oracle::OracleController,
    registry::RegistryController,
    storage_management::StorageManagementController,
    universal_account::UniversalAccountController,
    ContractController,
};
use controller::{mt::MtController, token::TokenController};
use near_sdk::{
    json_types::{I64, U64},
    serde_json, AccountId, NearToken,
};
use near_workspaces::{
    network::Sandbox,
    result::{ExecutionSuccess, ValueOrReceiptId},
    Account, DevNetwork, Worker,
};
use templar_common::{
    asset::FungibleAsset,
    dec,
    fee::{Fee, TimeBasedFee},
    interest_rate_strategy::InterestRateStrategy,
    market::{MarketConfiguration, PriceOracleConfiguration, YieldWeights},
    number::Decimal,
    oracle::pyth::{self, PriceIdentifier},
    registry::DeployMode,
    vault::{
        wad::{Wad, MAX_MANAGEMENT_FEE_WAD, MAX_PERFORMANCE_FEE_WAD},
        Fee as VaultFee, Fees as VaultFees, VaultConfiguration,
    },
};

pub const DEFAULT_COLLATERAL_PRICE_ID: PriceIdentifier = PriceIdentifier(hex_literal::hex!(
    "cccccccc232290221461220bd4e2acd1dcdfbc89c84092c93c18bdc7756c1588"
));
pub const DEFAULT_BORROW_PRICE_ID: PriceIdentifier = PriceIdentifier(hex_literal::hex!(
    "bbbbbbbbf4f61076456d1a73b14c7edc1cf5cef4f4d6193a33424288f11bd0f4"
));

pub mod controller;
pub mod partial;
pub mod pyth_price_id;

#[rstest::fixture]
pub async fn worker() -> Worker<Sandbox> {
    near_workspaces::sandbox_with_version("2.8.0")
        .await
        .unwrap()
}

pub fn to_price(price: f64) -> pyth::Price {
    pyth::Price {
        price: I64((price * 10000.0) as i64),
        conf: U64(0),
        expo: -4,
        publish_time: 0,
    }
}

pub async fn create_prefixed_account(
    prefix: &str,
    worker: &Worker<impl DevNetwork + 'static>,
) -> Account {
    let (genid, sk) = worker.generate_dev_account_credentials();
    let new_id: AccountId = format!("{prefix}{}", &genid.as_str()[prefix.len()..])
        .parse()
        .unwrap();
    worker
        .create_root_account_subaccount(new_id, sk)
        .await
        .unwrap()
        .unwrap()
}

#[macro_export]
macro_rules! accounts {
    ($w: expr, $($n:ident),*) => {
        $(let $n = $crate::create_prefixed_account(stringify!($n), &$w).await;)*
    };
}

#[macro_export]
macro_rules! setup_test {
    ($w:ident extract($($e:ident),*) accounts($($n:ident),*) config($f:expr) vconfig($v:expr)) => {
        $crate::accounts!($w, $($n),*);
        let s = $crate::setup_everything(&$w, $f, $v).await;
        ::tokio::join!(
            $(s.vault.init_account(&$n)),*
        );
        let $crate::SetupEverything { $($e,)* .. } = s;
    };
    ($w:ident extract($($e:ident),*) accounts($($n:ident),*) config($f:expr)) => {
        $crate::setup_test!($w extract($($e),*) accounts($($n),*) config($f) vconfig(|_| {}));
    };
    ($w:ident extract($($e:ident),*) accounts($($n:ident),*)) => {
        $crate::setup_test!($w extract($($e),*) accounts($($n),*) config(|_| {}) vconfig(|_| {}));
    };
}

pub fn market_configuration(
    price_oracle_id: AccountId,
    borrow_asset_id: AccountId,
    collateral_asset_id: AccountId,
    protocol_account_id: AccountId,
    yield_weights: YieldWeights,
) -> MarketConfiguration {
    MarketConfiguration {
        time_chunk_configuration: templar_common::time_chunk::TimeChunkConfiguration::new(1),
        borrow_asset: FungibleAsset::nep141(borrow_asset_id),
        collateral_asset: FungibleAsset::nep141(collateral_asset_id),
        price_oracle_configuration: PriceOracleConfiguration {
            account_id: price_oracle_id,
            collateral_asset_price_id: DEFAULT_COLLATERAL_PRICE_ID,
            collateral_asset_decimals: 24,
            borrow_asset_price_id: DEFAULT_BORROW_PRICE_ID,
            borrow_asset_decimals: 24,
            price_maximum_age_s: 60,
        },
        borrow_mcr_maintenance: Decimal::from_str("1.25").unwrap(),
        borrow_mcr_liquidation: Decimal::from_str("1.2").unwrap(),
        borrow_asset_maximum_usage_ratio: Decimal::from_str("0.99").unwrap(),
        borrow_origination_fee: Fee::Proportional(Decimal::from_str("0.1").unwrap()),
        borrow_interest_rate_strategy: InterestRateStrategy::piecewise(
            Decimal::ZERO,
            dec!("0.9"),
            dec!("0.04"),
            dec!("0.6"),
        )
        .unwrap(),
        borrow_maximum_duration_ms: None,
        borrow_range: (1, None).try_into().unwrap(),
        supply_range: (1, None).try_into().unwrap(),
        supply_withdrawal_range: (1, None).try_into().unwrap(),
        supply_withdrawal_fee: TimeBasedFee::zero(),
        liquidation_maximum_spread: Decimal::from_str("0.05").unwrap(),
        yield_weights,
        protocol_account_id,
    }
}

pub fn vault_configuration(
    owner_id: AccountId,
    curator_id: AccountId,
    guardian_id: AccountId,
    sentinel_id: AccountId,
    borrow_asset_id: AccountId,
    skim_recipient_id: AccountId,
    fee_recipient_id: AccountId,
) -> VaultConfiguration {
    VaultConfiguration {
        owner: owner_id,
        curator: curator_id,
        guardian: guardian_id,
        sentinel: sentinel_id,
        underlying_token: FungibleAsset::nep141(borrow_asset_id),
        initial_timelock_ns: templar_common::vault::MIN_TIMELOCK_NS.into(),
        fees: VaultFees {
            performance: VaultFee {
                fee: Wad::from(MAX_PERFORMANCE_FEE_WAD),
                recipient: fee_recipient_id.clone(),
            },
            management: VaultFee {
                fee: Wad::from(MAX_MANAGEMENT_FEE_WAD),
                recipient: fee_recipient_id,
            },
            max_total_assets_growth_rate: None,
        },
        skim_recipient: skim_recipient_id,
        name: "Vault".to_string(),
        symbol: "VAULT".to_string(),
        decimals: NonZero::new(24).unwrap(),
        restrictions: None,
        refresh_cooldown_ns: None,
        idle_resync_cooldown_ns: None,
        withdrawal_cooldown_ns: Some(0u64.into()),
    }
}

async fn compile_contract(p: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_WORKSPACE_DIR")).join(p);
    near_workspaces::compile_project(path.to_str().unwrap())
        .await
        .unwrap()
}

async fn read_contract(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_WORKSPACE_DIR"))
        .join("target/near/")
        .join(name)
        .join(name.to_owned() + ".wasm");

    std::fs::read(path).unwrap()
}

async fn get_contract(name: &str, path: &str) -> Vec<u8> {
    if std::env::var("TEST_CONTRACTS_PREBUILT").is_ok() {
        read_contract(name).await
    } else {
        compile_contract(path).await
    }
}

pub struct SetupEverything {
    pub c: UnifiedMarketController,
    pub protocol_yield_user: Account,
    pub insurance_yield_user: Account,
    pub vault: UnifiedVaultController,
    pub vault_owner: Account,
    pub vault_curator: Account,
    pub vault_guardian: Account,
    pub vault_sentinel: Account,
    pub skim_recipient: Account,
    pub fee_recipient: Account,
}

pub async fn setup_everything(
    worker: &Worker<Sandbox>,
    customize_market_configuration: impl FnOnce(&mut MarketConfiguration),
    customize_vault_configuration: impl FnOnce(&mut VaultConfiguration),
) -> SetupEverything {
    accounts!(
        worker,
        market,
        protocol_yield_user,
        insurance_yield_user,
        collateral_asset,
        borrow_asset,
        price_oracle,
        vault,
        vault_owner,
        vault_curator,
        vault_guardian,
        vault_sentinel,
        skim_recipient,
        fee_recipient
    );
    let mut config = market_configuration(
        price_oracle.id().clone(),
        borrow_asset.id().clone(),
        collateral_asset.id().clone(),
        protocol_yield_user.id().clone(),
        YieldWeights::new_with_supply_weight(8)
            .with_static(protocol_yield_user.id().clone(), 1)
            .with_static(insurance_yield_user.id().clone(), 1),
    );
    customize_market_configuration(&mut config);

    let mut vault_config = vault_configuration(
        vault_owner.id().clone(),
        vault_curator.id().clone(),
        vault_guardian.id().clone(),
        vault_sentinel.id().clone(),
        borrow_asset.id().clone(),
        skim_recipient.id().clone(),
        fee_recipient.id().clone(),
    );
    customize_vault_configuration(&mut vault_config);

    let (market, price_oracle, borrow_asset, collateral_asset, vault) = tokio::join!(
        MarketController::deploy(market, &config),
        OracleController::deploy(price_oracle),
        async {
            if config.borrow_asset.is_nep141(borrow_asset.id()) {
                TokenController::Ft {
                    controller: FtController::deploy(borrow_asset, "Borrow Asset", "BORROW").await,
                }
            } else {
                TokenController::Mt {
                    controller: MtController::deploy(borrow_asset).await,
                    token_id: "mt_borrow".into(),
                }
            }
        },
        async {
            if config.collateral_asset.is_nep141(collateral_asset.id()) {
                TokenController::Ft {
                    controller: FtController::deploy(
                        collateral_asset,
                        "Collateral Asset",
                        "COLLATERAL",
                    )
                    .await,
                }
            } else {
                TokenController::Mt {
                    controller: MtController::deploy(collateral_asset).await,
                    token_id: "mt_collateral".into(),
                }
            }
        },
        VaultController::deploy(vault, &vault_config)
    );

    let c =
        UnifiedMarketController::new(market, config, price_oracle, borrow_asset, collateral_asset);

    c.set_borrow_asset_price(1.0).await;
    c.set_collateral_asset_price(1.0).await;

    let v = UnifiedVaultController::new(vault, vault_config, c.clone());

    let mkt = c.market.contract().as_account();
    // Asset opt-ins.
    tokio::join!(
        c.storage_deposits(mkt),
        c.init_account(&protocol_yield_user),
        c.init_account(&insurance_yield_user),
        v.storage_deposits(v.vault.contract().as_account()),
        v.storage_deposits(&skim_recipient),
        v.storage_deposits(&fee_recipient),
    );

    v.setup_caps(&vault_owner, &[mkt.id().clone()], u128::MAX)
        .await;

    SetupEverything {
        c,
        protocol_yield_user,
        insurance_yield_user,
        vault: v,
        vault_owner,
        vault_curator,
        vault_guardian,
        vault_sentinel,
        skim_recipient,
        fee_recipient,
    }
}

pub async fn setup_registry(worker: &Worker<Sandbox>) -> RegistryController {
    accounts!(worker, registry);

    let r = RegistryController::new(registry).await;

    let wasm = controller::market::load_wasm().await;

    let cost_per_byte = NearToken::from_near(1).saturating_div(10 * 1_000);
    let deployment_cost = cost_per_byte.saturating_mul(wasm.len() as u128);

    r.add_version(
        r.contract.as_account(),
        deployment_cost,
        "market@0.0.0",
        DeployMode::GlobalHash,
        wasm,
    )
    .await;

    r
}

pub fn print_execution(e: &ExecutionSuccess) {
    eprintln!("Execution:");
    eprintln!("Total gas burnt: {}", e.total_gas_burnt);
    eprintln!("Executor: {}", e.outcome().executor_id);
    eprintln!("Receipts:");
    for (i, receipt) in e.receipt_outcomes().iter().enumerate() {
        eprintln!("\tReceipt #{i}:");
        eprintln!("\tExecutor: {}", receipt.executor_id);
        eprintln!("\tGas burnt: {}", receipt.gas_burnt);
        if !receipt.logs.is_empty() {
            eprintln!("\tLogs:");
            for log in &receipt.logs {
                eprintln!("\t\t{log}");
            }
        }
        match receipt.clone().into_result() {
            Ok(ValueOrReceiptId::Value(value)) => {
                if let Some(s) = value
                    .json::<serde_json::Value>()
                    .ok()
                    .and_then(|v| serde_json::to_string(&v).ok())
                {
                    eprintln!("\tReturn value: {s}");
                }
            }
            Err(e) => {
                eprintln!("\tError: {e:?}");
            }
            _ => {}
        }
        eprintln!();
    }
}
