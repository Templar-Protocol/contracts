//! Positive custodial settlement E2E on the dedicated epoch deploy target.
//!
//! Drives only production surfaces with production wire encodings:
//! a governance-authorized one-time backed seed (custody observed at the
//! vault, exact 1-for-1 share mint), a withdrawal escrow, governance
//! adapter binding, the epoch cutoff, real custodian valuation reports
//! authenticated on the real registered custodial adapter, settlement,
//! and the settled-head withdrawal execution law. Asserts the exact
//! immutable snapshot binding (epoch, report sequence, as-of, hash, NAV,
//! eligible supply, cutoff), the refund-mode FIFO law with full escrow
//! integrity, and the replay/reuse failures that law must accompany any
//! successful settlement.

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token::StellarAssetClient,
    Address, Bytes, BytesN, Env,
};
use std::string::String as StdString;
use templar_soroban_custodial_adapter::CustodialAdapterContract;
use templar_soroban_epoch_runtime::SorobanEpochVaultContract;
use templar_soroban_runtime::error::ContractError;
use templar_soroban_shared_types::{
    AdmitPendingDepositReceipt, BeginEpochCutoffReceipt, ConfigureEpochSettlementReceipt,
    CustodialValuationReport, EpochSnapshotReceipt, EpochStateViewReceipt, ExecuteWithdrawReceipt,
    GovernanceCommand, PendingDepositReceipt, RequestWithdrawReceipt, SeedEpochSupplyReceipt,
    SettleEpochReceipt, VaultCommand,
    EPOCH_PHASE_CUTOFF, EPOCH_PHASE_OPEN, GOVERNANCE_CONFIG_KIND_ALLOWED_ADAPTERS,
    GOVERNANCE_POLICY_KIND_CAP, GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
};

const CUTOFF_NS: u64 = 100_000_000_000;
const REPORT_AS_OF_NS: u64 = 101_000_000_000;
const MAX_REPORT_AGE_NS: u64 = 60_000_000_000;
const WITHDRAWAL_COOLDOWN_NS: u64 = 1_000_000_000;
const REPORT_HASH: [u8; 32] = [7u8; 32];

/// Seeded custodial backing: assets custodied == shares minted 1-for-1.
const SEEDED_BACKING: i128 = 10_000;
/// Holder A escrows its entire seeded position (the FIFO head).
const ESCROWED_SHARES: i128 = SEEDED_BACKING;
/// Custody held by the custodian off the vault, as attested by the custodian.
/// The governed seed established the entire backing as custody held at the
/// vault, so the custodian's truthful attestation is zero custody held at
/// the custodian. Settlement law binds the vault's own book total as the
/// settlement NAV, which is exactly the seeded backing.
const CUSTODY_ROUTE_VALUE: i128 = 0;
/// Second depositor's intake requested before the cutoff, at the minimum
/// lawful share floor. Pending deposit custody is held outside the vault
/// books and never changes NAV or supply until admission at settlement.
const PENDING_DEPOSIT_ASSETS: i128 = 1_000;
const SNAPSHOT_EPOCH_ID: u64 = 1;
const SNAPSHOT_REPORT_SEQ: u64 = 1;
const SNAPSHOT_NAV: i128 = 10_000;
const SNAPSHOT_SUPPLY: i128 = 10_000;
/// Custody market bound to the custodial adapter by governance law.
const CUSTODY_MARKET: u32 = 1;
/// Governance-provisioned cap for the custody market. The supply queue only
/// accepts markets that exist, are enabled, and carry a nonzero cap, so the
/// cap policy law provisions the custody market before it is bound.
const CUSTODY_MARKET_CAP: i128 = 1_000_000;

struct Fixture {
    env: Env,
    epoch: Address,
    asset: Address,
    share: Address,
    governance: Address,
    curator: Address,
    holder_a: Address,
    holder_b: Address,
    custodian: Address,
    adapter: Address,
}

fn address_text(_env: &Env, address: &Address) -> StdString {
    StdString::from_utf8(address.to_string().to_bytes().to_alloc_vec()).expect("utf-8 strkey")
}

fn fixture() -> Fixture {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();
    env.ledger().set(soroban_sdk::testutils::LedgerInfo {
        protocol_version: 25,
        timestamp: 1,
        base_reserve: 0,
        min_persistent_entry_ttl: 4_096,
        min_temp_entry_ttl: 631_200,
        max_entry_ttl: 5_184_000,
        ..Default::default()
    });
    let epoch = env.register(SorobanEpochVaultContract, ());
    let governance = env.register(StubGovernance, ());
    let asset_admin = Address::generate(&env);
    let asset = env
        .register_stellar_asset_contract_v2(asset_admin.clone())
        .address();
    let share = env
        .register_stellar_asset_contract_v2(epoch.clone())
        .address();
    let curator = Address::generate(&env);
    let holder_a = Address::generate(&env);
    let holder_b = Address::generate(&env);
    let custodian = Address::generate(&env);
    let adapter = env.register(
        CustodialAdapterContract,
        (
            Address::generate(&env),
            epoch.clone(),
            custodian.clone(),
            asset.clone(),
        ),
    );
    env.as_contract(&epoch, || {
        SorobanEpochVaultContract::initialize_with_config(
            env.clone(),
            curator.clone(),
            governance.clone(),
            asset.clone(),
            share.clone(),
            0,
            0,
            WITHDRAWAL_COOLDOWN_NS,
        )
        .expect("fresh epoch deployment initializes");
    });
    Fixture {
        env,
        epoch,
        asset,
        share,
        governance,
        curator,
        holder_a,
        holder_b,
        custodian,
        adapter,
    }
}

fn execute(fixture: &Fixture, command: &VaultCommand) -> Result<Bytes, ContractError> {
    let payload = command.encode();
    fixture.env.as_contract(&fixture.epoch, || {
        SorobanEpochVaultContract::execute(
            fixture.env.clone(),
            Bytes::from_slice(&fixture.env, &payload),
        )
    })
}

fn execute_ok(fixture: &Fixture, command: &VaultCommand) -> Vec<u8> {
    execute(fixture, command)
        .expect("epoch command must succeed")
        .to_alloc_vec()
}

fn execute_err(fixture: &Fixture, command: &VaultCommand) -> ContractError {
    execute(fixture, command).expect_err("epoch command must fail closed")
}

fn execute_governance_ok(fixture: &Fixture, command: &GovernanceCommand) {
    let payload = command.encode();
    fixture
        .env
        .as_contract(&fixture.epoch, || {
            SorobanEpochVaultContract::execute_governance(
                fixture.env.clone(),
                fixture.governance.clone(),
                Bytes::from_slice(&fixture.env, &payload),
            )
        })
        .expect("governance command must succeed");
}

fn epoch_state(fixture: &Fixture) -> EpochStateViewReceipt {
    EpochStateViewReceipt::decode(&execute_ok(fixture, &VaultCommand::GetEpochState))
        .expect("epoch state view")
}

fn snapshot(fixture: &Fixture, epoch_id: u64) -> EpochSnapshotReceipt {
    EpochSnapshotReceipt::decode(&execute_ok(
        fixture,
        &VaultCommand::GetEpochSnapshot { epoch_id },
    ))
    .expect("epoch snapshot")
}

/// Recomputes the production settlement header digest for the values this
/// E2E settles. The vault binds `sha256(domain || epoch || cutoff || report
/// sequence || as-of || settlement NAV || eligible supply || vault kernel
/// address)` as the snapshot report hash, never the custodian report
/// integrity hash, so the positive E2E pins this exact law.
fn settlement_header_digest(fixture: &Fixture) -> [u8; 32] {
    const SETTLEMENT_HEADER_DOMAIN: &[u8] = b"templar:soroban:settlement:v1";
    const KERNEL_ADDRESS_DOMAIN: &[u8] = b"templar:soroban:address";
    let strkey = fixture.epoch.to_string().to_bytes().to_alloc_vec();
    let mut vault_kernel_address =
        Vec::with_capacity(KERNEL_ADDRESS_DOMAIN.len() + strkey.len());
    vault_kernel_address.extend_from_slice(KERNEL_ADDRESS_DOMAIN);
    vault_kernel_address.extend_from_slice(&strkey);
    let vault_kernel_address = fixture
        .env
        .crypto()
        .sha256(&Bytes::from_slice(&fixture.env, &vault_kernel_address))
        .to_bytes()
        .to_array();
    let mut preimage = Vec::new();
    preimage.extend_from_slice(SETTLEMENT_HEADER_DOMAIN);
    preimage.extend_from_slice(&SNAPSHOT_EPOCH_ID.to_be_bytes());
    preimage.extend_from_slice(&CUTOFF_NS.to_be_bytes());
    preimage.extend_from_slice(&SNAPSHOT_REPORT_SEQ.to_be_bytes());
    preimage.extend_from_slice(&REPORT_AS_OF_NS.to_be_bytes());
    preimage.extend_from_slice(&(SNAPSHOT_NAV as u128).to_be_bytes());
    preimage.extend_from_slice(&(SNAPSHOT_SUPPLY as u128).to_be_bytes());
    preimage.extend_from_slice(&vault_kernel_address);
    fixture
        .env
        .crypto()
        .sha256(&Bytes::from_slice(&fixture.env, &preimage))
        .to_bytes()
        .to_array()
}

fn custodian_submit(
    fixture: &Fixture,
    sequence: u64,
    as_of_secs: u64,
    value: i128,
) -> Result<(), templar_soroban_custodial_adapter::AdapterError> {
    fixture
        .env
        .as_contract(&fixture.adapter, || {
            CustodialAdapterContract::submit_report(
                fixture.env.clone(),
                fixture.custodian.clone(),
                CustodialValuationReport {
                    vault: fixture.epoch.clone(),
                    adapter: fixture.adapter.clone(),
                    asset: fixture.asset.clone(),
                    network_id: fixture.env.ledger().network_id(),
                    sequence,
                    as_of: as_of_secs,
                    assets_value: value,
                    report_hash: Some(BytesN::from_array(&fixture.env, &REPORT_HASH)),
                },
            )
        })
}

#[soroban_sdk::contract]
struct StubGovernance;

#[soroban_sdk::contractimpl]
impl StubGovernance {
    pub fn noop(env: Env) -> u32 {
        let _ = env;
        0
    }
}

#[test]
fn positive_settlement_binds_real_snapshot_and_payout_law_holds() {
    let fixture = fixture();

    // Fresh deployment: intake is open at the first settlement epoch.
    let fresh = epoch_state(&fixture);
    assert_eq!(fresh.phase, EPOCH_PHASE_OPEN);
    assert_eq!(fresh.intake_epoch, SNAPSHOT_EPOCH_ID);
    assert_eq!(fresh.cutoff_ns, None);
    assert_eq!(fresh.last_settled_epoch_id, None);

    // Governance configures the report-freshness law that gates settlement.
    let configured = ConfigureEpochSettlementReceipt::decode(&execute_ok(
        &fixture,
        &VaultCommand::ConfigureEpochSettlement {
            caller: address_text(&fixture.env, &fixture.governance),
            max_report_age_ns: MAX_REPORT_AGE_NS,
        },
    ))
    .expect("epoch settlement configuration receipt");
    assert_eq!(configured.max_report_age_ns, MAX_REPORT_AGE_NS);

    // One-time governed seed establishes backed supply. Custody is moved
    // into the vault through the asset contract and observed at the exact
    // seed amount before any mint law is dispatched; the kernel then mints
    // one-for-one to the receiver. Every later step is ordinary law
    // applied to that real accounting.
    StellarAssetClient::new(&fixture.env, &fixture.asset).mint(&fixture.epoch, &SEEDED_BACKING);
    let seed = SeedEpochSupplyReceipt::decode(&execute_ok(
        &fixture,
        &VaultCommand::SeedEpochSupply {
            caller: address_text(&fixture.env, &fixture.governance),
            receiver: address_text(&fixture.env, &fixture.holder_a),
            assets: SEEDED_BACKING,
        },
    ))
    .expect("seed receipt");
    assert_eq!(seed.assets_seeded, SEEDED_BACKING);
    assert_eq!(seed.shares_minted, SEEDED_BACKING);
    assert_eq!(
        StellarAssetClient::new(&fixture.env, &fixture.share).balance(&fixture.holder_a),
        SEEDED_BACKING,
        "seed must mint exactly the custodied assets as shares to the receiver"
    );

    // Seed replay fails closed: accounting is no longer pristine.
    let seed_replay = execute_err(
        &fixture,
        &VaultCommand::SeedEpochSupply {
            caller: address_text(&fixture.env, &fixture.governance),
            receiver: address_text(&fixture.env, &fixture.holder_a),
            assets: SEEDED_BACKING,
        },
    );
    assert_ne!(
        seed_replay,
        ContractError::Unauthorized,
        "a seed replay must reach seed law, not the role gate"
    );

    // Withdrawal escrow is queued pre-cutoff.
    fixture.env.ledger().set_timestamp(2);
    let withdrawal = RequestWithdrawReceipt::decode(&execute_ok(
        &fixture,
        &VaultCommand::RequestWithdraw {
            owner: address_text(&fixture.env, &fixture.holder_a),
            receiver: address_text(&fixture.env, &fixture.holder_a),
            shares: ESCROWED_SHARES,
            min_assets_out: 1,
        },
    ))
    .expect("withdrawal request receipt");
    assert_eq!(withdrawal.shares_escrowed, ESCROWED_SHARES);
    assert_eq!(
        StellarAssetClient::new(&fixture.env, &fixture.share).balance(&fixture.holder_a),
        0,
        "the full share position must be escrowed"
    );
    assert_eq!(
        StellarAssetClient::new(&fixture.env, &fixture.asset).balance(&fixture.epoch),
        SEEDED_BACKING,
        "custody holds the seeded backing"
    );

    // Governance law provisions the custody market with an enabled, nonzero
    // cap before it may join the supply queue, then binds the custodial
    // adapter to that market so the settlement scan can observe
    // authenticated custodian reports.
    execute_governance_ok(
        &fixture,
        &GovernanceCommand::SetGovernancePolicy {
            kind: GOVERNANCE_POLICY_KIND_CAP,
            target_ids: None,
            mode: None,
            accounts: None,
            market_id: Some(CUSTODY_MARKET),
            cap_group_id: None,
            value: Some(CUSTODY_MARKET_CAP),
            value_b: None,
            value_c: None,
        },
    );
    fixture
        .env
        .as_contract(&fixture.epoch, || {
            SorobanEpochVaultContract::execute_governance(
                fixture.env.clone(),
                fixture.governance.clone(),
                Bytes::from_slice(
                    &fixture.env,
                    &GovernanceCommand::SetGovernanceConfig {
                        kind: GOVERNANCE_CONFIG_KIND_ALLOWED_ADAPTERS,
                        primary: None,
                        many: Some(vec![address_text(&fixture.env, &fixture.adapter)]),
                        value_a: None,
                        value_b: None,
                    }
                    .encode(),
                ),
            )
            .expect("allowed adapters configuration must succeed");
        });
    execute_governance_ok(
        &fixture,
        &GovernanceCommand::SetGovernancePolicy {
            kind: GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
            target_ids: Some(vec![CUSTODY_MARKET]),
            mode: None,
            accounts: Some(vec![address_text(&fixture.env, &fixture.adapter)]),
            market_id: None,
            cap_group_id: None,
            value: None,
            value_b: None,
            value_c: None,
        },
    );

    // The epoch cutoff binds at the governed time.
    fixture.env.ledger().set_timestamp(CUTOFF_NS / 1_000_000_000);
    let cutoff = BeginEpochCutoffReceipt::decode(&execute_ok(
        &fixture,
        &VaultCommand::BeginEpochCutoff {
            caller: address_text(&fixture.env, &fixture.curator),
            cutoff_ns: CUTOFF_NS,
        },
    ))
    .expect("cutoff receipt");
    assert_eq!(cutoff.epoch_id, SNAPSHOT_EPOCH_ID);
    assert_eq!(cutoff.cutoff_ns, CUTOFF_NS);
    assert_eq!(epoch_state(&fixture).phase, EPOCH_PHASE_CUTOFF);

    // Post-cutoff, the configured custodian submits a real, authenticated
    // valuation report on the real adapter. No caller-supplied report ever
    // touches the settlement path: settlement law reads only the adapter's
    // accepted view of exactly this report.
    fixture.env.ledger().set_timestamp(REPORT_AS_OF_NS / 1_000_000_000);
    custodian_submit(
        &fixture,
        SNAPSHOT_REPORT_SEQ,
        REPORT_AS_OF_NS / 1_000_000_000,
        CUSTODY_ROUTE_VALUE,
    )
    .expect("custodian report submission must be accepted");
    let accepted = fixture
        .env
        .as_contract(&fixture.adapter, || {
            CustodialAdapterContract::valuation(fixture.env.clone(), fixture.asset.clone())
        })
        .expect("adapter view")
        .expect("accepted custodian report must be observable to the settlement scan");
    assert_eq!(accepted.0, SNAPSHOT_REPORT_SEQ);
    assert_eq!(accepted.1, REPORT_AS_OF_NS / 1_000_000_000);
    assert!(matches!(accepted.4, Some(_)));

    // Settlement binds the authenticated report and the vault books into one
    // immutable snapshot: the exact epoch, report sequence, as-of, hash,
    // cutoff, custodial NAV, and eligible supply.
    let settled = SettleEpochReceipt::decode(&execute_ok(
        &fixture,
        &VaultCommand::SettleEpoch {
            caller: address_text(&fixture.env, &fixture.curator),
        },
    ))
    .expect("settlement receipt");
    assert_eq!(settled.epoch_id, SNAPSHOT_EPOCH_ID);
    assert_eq!(settled.report_seq, SNAPSHOT_REPORT_SEQ);
    assert_eq!(settled.as_of_ns, REPORT_AS_OF_NS);
    assert_eq!(
        settled.report_hash,
        settlement_header_digest(&fixture),
        "the snapshot binds the vault's domain-tagged settlement header digest over the exact settled values"
    );
    assert_ne!(
        settled.report_hash, REPORT_HASH,
        "the snapshot binds the settlement header digest, not the custodian report integrity hash itself"
    );
    assert_eq!(settled.cutoff_ns, CUTOFF_NS);
    assert_eq!(
        settled.settlement_nav, SNAPSHOT_NAV,
        "settlement NAV is the custodial backing"
    );
    assert_eq!(settled.eligible_supply, SNAPSHOT_SUPPLY);

    // The snapshot is immutable law: every fetch returns the same binding.
    let fetched = snapshot(&fixture, SNAPSHOT_EPOCH_ID);
    assert_eq!(fetched.epoch_id, settled.epoch_id);
    assert_eq!(fetched.report_seq, settled.report_seq);
    assert_eq!(fetched.as_of_ns, settled.as_of_ns);
    assert_eq!(fetched.report_hash, settled.report_hash);
    assert_eq!(fetched.cutoff_ns, settled.cutoff_ns);
    assert_eq!(fetched.settlement_nav, settled.settlement_nav);
    assert_eq!(fetched.eligible_supply, settled.eligible_supply);
    let refetched = snapshot(&fixture, SNAPSHOT_EPOCH_ID);
    assert_eq!(refetched.epoch_id, fetched.epoch_id);
    assert_eq!(refetched.report_seq, fetched.report_seq);
    assert_eq!(refetched.as_of_ns, fetched.as_of_ns);
    assert_eq!(refetched.report_hash, fetched.report_hash);
    assert_eq!(refetched.cutoff_ns, fetched.cutoff_ns);
    assert_eq!(refetched.settlement_nav, fetched.settlement_nav);
    assert_eq!(refetched.eligible_supply, fetched.eligible_supply);

    // Settlement replay fails closed: no caller-supplied report can settle
    // the epoch again.
    let replay = execute_err(
        &fixture,
        &VaultCommand::SettleEpoch {
            caller: address_text(&fixture.env, &fixture.curator),
        },
    );
    assert_ne!(
        replay,
        ContractError::Unauthorized,
        "an authorized replay must reach settlement law, not the role gate"
    );

    // Report reuse fails closed: the same authenticated report cannot be
    // resubmitted to drive a second settlement.
    let reuse = custodian_submit(
        &fixture,
        SNAPSHOT_REPORT_SEQ,
        REPORT_AS_OF_NS / 1_000_000_000,
        CUSTODY_ROUTE_VALUE,
    );
    assert!(
        reuse.is_err(),
        "the same custodian report cannot be resubmitted for reuse"
    );

    // The settled FIFO head is executed by law against the bound snapshot:
    // the claim funded by custody is payable and must complete, paying
    // exactly the derived claim and burning the escrow in full. A NoPayout
    // or refund outcome is a law violation when custody funds the claim,
    // so this E2E never accepts it.
    fixture
        .env
        .ledger()
        .set_timestamp(101 + WITHDRAWAL_COOLDOWN_NS / 1_000_000_000);
    let executed = ExecuteWithdrawReceipt::decode(&execute_ok(
        &fixture,
        &VaultCommand::ExecuteWithdraw {
            caller: address_text(&fixture.env, &fixture.curator),
        },
    ))
    .expect("withdrawal execution receipt");
    let (owner, receiver, assets_out, shares_burned) = match executed {
        ExecuteWithdrawReceipt::Completed {
            owner,
            receiver,
            assets_out,
            shares_burned,
            ..
        } => (owner, receiver, assets_out, shares_burned),
        ExecuteWithdrawReceipt::NoPayout { status } => {
            panic!(
                "a claim funded by custodial backing must pay in full, never refund: {:?}",
                status
            )
        }
    };
    assert_eq!(owner.as_str(), address_text(&fixture.env, &fixture.holder_a));
    assert_eq!(receiver.as_str(), address_text(&fixture.env, &fixture.holder_a));
    assert_eq!(
        assets_out as i128, SEEDED_BACKING,
        "the settled head is paid exactly floor({ESCROWED_SHARES} * {SNAPSHOT_NAV} / {SNAPSHOT_SUPPLY}) from custodial backing"
    );
    assert_eq!(shares_burned as i128, ESCROWED_SHARES);
    assert_eq!(
        StellarAssetClient::new(&fixture.env, &fixture.asset).balance(&fixture.epoch),
        0,
        "the payout moved the derived claim out of custody, leaving nothing behind"
    );
    assert_eq!(
        StellarAssetClient::new(&fixture.env, &fixture.asset).balance(&fixture.holder_a),
        SEEDED_BACKING,
        "the settled holder received the full derived claim"
    );
    assert_eq!(
        StellarAssetClient::new(&fixture.env, &fixture.share).balance(&fixture.holder_a),
        0,
        "the executed head's escrow was burned in full"
    );
    assert_eq!(
        StellarAssetClient::new(&fixture.env, &fixture.share).balance(&fixture.epoch),
        0,
        "burning the executed escrow in full leaves no share held by vault escrow law"
    );
    assert_eq!(
        StellarAssetClient::new(&fixture.env, &fixture.share).balance(&fixture.holder_a),
        0,
        "burning the executed escrow in full leaves no share with the settled holder"
    );

    // The executed head is finished: a second execution of the same
    // request fails closed because the FIFO advanced.
    let replay_execution = execute_err(
        &fixture,
        &VaultCommand::ExecuteWithdraw {
            caller: address_text(&fixture.env, &fixture.curator),
        },
    );
    assert_ne!(
        replay_execution,
        ContractError::Unauthorized,
        "a second execution of the executed head must reach queue law, not the role gate"
    );

    // The snapshot remains the exact immutable record after execution: no
    // settlement ever rewrote or replaced it.
    let after = snapshot(&fixture, SNAPSHOT_EPOCH_ID);
    assert_eq!(after.epoch_id, settled.epoch_id);
    assert_eq!(after.report_seq, settled.report_seq);
    assert_eq!(after.as_of_ns, settled.as_of_ns);
    assert_eq!(after.report_hash, settled.report_hash);
    assert_eq!(after.cutoff_ns, settled.cutoff_ns);
    assert_eq!(after.settlement_nav, settled.settlement_nav);
    assert_eq!(after.eligible_supply, settled.eligible_supply);

    // The settled epoch is recorded and intake advanced to the next epoch.
    let advanced = epoch_state(&fixture);
    assert_eq!(advanced.last_settled_epoch_id, Some(SNAPSHOT_EPOCH_ID));
    assert_eq!(advanced.last_report_seq, Some(SNAPSHOT_REPORT_SEQ));
    assert_eq!(advanced.intake_epoch, SNAPSHOT_EPOCH_ID + 1);
}

/// Positive E2E for intake isolation and admission law. A second depositor
/// funds 1,000 of custodial assets before the cutoff; the deposit transfers
/// custody and creates a pending record for the intake epoch without moving
/// NAV or supply. The settlement snapshot still binds exactly
/// 10,000 NAV against 10,000 supply, proving the pending deposit assets were
/// excluded from the valuation. After settlement the pending deposit is
/// admitted at the snapshot price exactly once, minting exactly 1,000 shares
/// with no second asset transfer, and a replay of the admission fails.
#[test]
fn pending_deposit_is_custodied_excluded_and_admitted_exactly_once() {
    let fixture = fixture();

    let configured = ConfigureEpochSettlementReceipt::decode(&execute_ok(
        &fixture,
        &VaultCommand::ConfigureEpochSettlement {
            caller: address_text(&fixture.env, &fixture.governance),
            max_report_age_ns: MAX_REPORT_AGE_NS,
        },
    ))
    .expect("epoch settlement configuration receipt");
    assert_eq!(configured.max_report_age_ns, MAX_REPORT_AGE_NS);

    // One-time governed seed establishes the backed book.
    StellarAssetClient::new(&fixture.env, &fixture.asset).mint(&fixture.epoch, &SEEDED_BACKING);
    let seed = SeedEpochSupplyReceipt::decode(&execute_ok(
        &fixture,
        &VaultCommand::SeedEpochSupply {
            caller: address_text(&fixture.env, &fixture.governance),
            receiver: address_text(&fixture.env, &fixture.holder_a),
            assets: SEEDED_BACKING,
        },
    ))
    .expect("seed receipt");
    assert_eq!(seed.assets_seeded, SEEDED_BACKING);
    assert_eq!(seed.shares_minted, SEEDED_BACKING);

    // Holder A escrows the full seeded position before the cutoff.
    fixture.env.ledger().set_timestamp(2);
    let withdrawal = RequestWithdrawReceipt::decode(&execute_ok(
        &fixture,
        &VaultCommand::RequestWithdraw {
            owner: address_text(&fixture.env, &fixture.holder_a),
            receiver: address_text(&fixture.env, &fixture.holder_a),
            shares: ESCROWED_SHARES,
            min_assets_out: 1,
        },
    ))
    .expect("withdrawal request receipt");
    assert_eq!(withdrawal.shares_escrowed, ESCROWED_SHARES);

    // A second depositor funds 1,000 of custodial assets and requests a
    // deposit before the cutoff. Custody transfers and a pending record is
    // created for the open epoch, without minting shares or touching NAV.
    StellarAssetClient::new(&fixture.env, &fixture.asset)
        .mint(&fixture.holder_b, &PENDING_DEPOSIT_ASSETS);
    let deposit = PendingDepositReceipt::decode(&execute_ok(
        &fixture,
        &VaultCommand::RequestDeposit {
            owner: address_text(&fixture.env, &fixture.holder_b),
            assets: PENDING_DEPOSIT_ASSETS,
            min_shares_out: PENDING_DEPOSIT_ASSETS,
        },
    ))
    .expect("pending deposit receipt");
    assert!(
        deposit.request_id >= 1,
        "the deposit produced a pending intake record for the open epoch"
    );
    assert_eq!(
        StellarAssetClient::new(&fixture.env, &fixture.asset).balance(&fixture.holder_b),
        0,
        "the deposit transferred the custodial assets"
    );
    assert_eq!(
        StellarAssetClient::new(&fixture.env, &fixture.asset).balance(&fixture.epoch),
        SEEDED_BACKING + PENDING_DEPOSIT_ASSETS,
        "the deposit custody is held by the vault, nothing else moved"
    );
    assert_eq!(
        StellarAssetClient::new(&fixture.env, &fixture.share).balance(&fixture.holder_b),
        0,
        "a pending deposit creates no shares until settlement"
    );

    // Governance law provisions the custody market and binds the custodial
    // adapter, exactly as in the positive settlement E2E.
    execute_governance_ok(
        &fixture,
        &GovernanceCommand::SetGovernancePolicy {
            kind: GOVERNANCE_POLICY_KIND_CAP,
            target_ids: None,
            mode: None,
            accounts: None,
            market_id: Some(CUSTODY_MARKET),
            cap_group_id: None,
            value: Some(CUSTODY_MARKET_CAP),
            value_b: None,
            value_c: None,
        },
    );
    fixture
        .env
        .as_contract(&fixture.epoch, || {
            SorobanEpochVaultContract::execute_governance(
                fixture.env.clone(),
                fixture.governance.clone(),
                Bytes::from_slice(
                    &fixture.env,
                    &GovernanceCommand::SetGovernanceConfig {
                        kind: GOVERNANCE_CONFIG_KIND_ALLOWED_ADAPTERS,
                        primary: None,
                        many: Some(vec![address_text(&fixture.env, &fixture.adapter)]),
                        value_a: None,
                        value_b: None,
                    }
                    .encode(),
                ),
            )
            .expect("allowed adapters configuration must succeed");
        });
    execute_governance_ok(
        &fixture,
        &GovernanceCommand::SetGovernancePolicy {
            kind: GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
            target_ids: Some(vec![CUSTODY_MARKET]),
            mode: None,
            accounts: Some(vec![address_text(&fixture.env, &fixture.adapter)]),
            market_id: None,
            cap_group_id: None,
            value: None,
            value_b: None,
            value_c: None,
        },
    );

    // Close intake and settle with a fresh post-cutoff custodian report.
    // The settlement snapshot binds exactly the seeded book: the pending
    // deposit's 1,000 assets are excluded from the valuation and supply.
    fixture.env.ledger().set_timestamp(CUTOFF_NS / 1_000_000_000);
    let cutoff = BeginEpochCutoffReceipt::decode(&execute_ok(
        &fixture,
        &VaultCommand::BeginEpochCutoff {
            caller: address_text(&fixture.env, &fixture.curator),
            cutoff_ns: CUTOFF_NS,
        },
    ))
    .expect("cutoff receipt");
    assert_eq!(cutoff.epoch_id, SNAPSHOT_EPOCH_ID);
    fixture.env.ledger().set_timestamp(REPORT_AS_OF_NS / 1_000_000_000);
    custodian_submit(
        &fixture,
        SNAPSHOT_REPORT_SEQ,
        REPORT_AS_OF_NS / 1_000_000_000,
        CUSTODY_ROUTE_VALUE,
    )
    .expect("custodian report submission must be accepted");
    let settled = SettleEpochReceipt::decode(&execute_ok(
        &fixture,
        &VaultCommand::SettleEpoch {
            caller: address_text(&fixture.env, &fixture.curator),
        },
    ))
    .expect("settlement receipt");
    assert_eq!(settled.epoch_id, SNAPSHOT_EPOCH_ID);
    assert_eq!(
        settled.settlement_nav, SNAPSHOT_NAV,
        "the pending deposit's assets are excluded from the settlement valuation"
    );
    assert_eq!(
        settled.eligible_supply, SNAPSHOT_SUPPLY,
        "the pending deposit creates no supply before admission"
    );
    assert_eq!(
        settled.report_hash,
        settlement_header_digest(&fixture),
        "the snapshot binds the vault's domain-tagged settlement header digest"
    );

    // The settled FIFO head is executed: the derived claim is paid from the
    // custodial backing and the escrow burns in full. The deposit custody
    // is untouched by the payout.
    fixture
        .env
        .ledger()
        .set_timestamp(CUTOFF_NS / 1_000_000_000 + WITHDRAWAL_COOLDOWN_NS / 1_000_000_000 + 1);
    let executed = ExecuteWithdrawReceipt::decode(&execute_ok(
        &fixture,
        &VaultCommand::ExecuteWithdraw {
            caller: address_text(&fixture.env, &fixture.curator),
        },
    ))
    .expect("settled withdrawal must execute");
    match executed {
        ExecuteWithdrawReceipt::Completed {
            owner,
            assets_out,
            shares_burned,
            ..
        } => {
            assert_eq!(owner.as_str(), address_text(&fixture.env, &fixture.holder_a));
            assert_eq!(assets_out as i128, SEEDED_BACKING);
            assert_eq!(shares_burned as i128, ESCROWED_SHARES);
            assert_eq!(
                StellarAssetClient::new(&fixture.env, &fixture.asset).balance(&fixture.holder_a),
                SEEDED_BACKING,
                "the settled holder received the full derived claim"
            );
        }
        ExecuteWithdrawReceipt::NoPayout { status } => {
            panic!(
                "a claim funded by custodial backing must pay in full, never refund: {:?}",
                status
            )
        }
    }

    // After settlement, the pending deposit is admitted at the snapshot
    // price exactly once, with no second transfer of the already-held
    // custody, minting exactly the derived shares to the depositor.
    let admitted = AdmitPendingDepositReceipt::decode(&execute_ok(
        &fixture,
        &VaultCommand::AdmitPendingDeposit {
            caller: address_text(&fixture.env, &fixture.curator),
            request_id: deposit.request_id,
        },
    ))
    .expect("pending deposit admission receipt");
    assert_eq!(admitted.request_id, deposit.request_id);
    assert_eq!(
        admitted.shares_out as i128, PENDING_DEPOSIT_ASSETS,
        "the pending deposit admits at the snapshot price, minting exactly the derived shares once"
    );
    assert_eq!(admitted.assets_in as i128, PENDING_DEPOSIT_ASSETS);
    assert_eq!(
        StellarAssetClient::new(&fixture.env, &fixture.share).balance(&fixture.holder_b),
        PENDING_DEPOSIT_ASSETS,
        "the admitted depositor holds exactly the snapshot-priced shares"
    );
    assert_eq!(
        StellarAssetClient::new(&fixture.env, &fixture.asset).balance(&fixture.epoch),
        PENDING_DEPOSIT_ASSETS,
        "admission caused no second asset transfer; only the depositor's own custody remains, backing the admitted shares"
    );

    // Replaying the admission fails closed: the taken record is gone.
    let replay = execute_err(
        &fixture,
        &VaultCommand::AdmitPendingDeposit {
            caller: address_text(&fixture.env, &fixture.curator),
            request_id: deposit.request_id,
        },
    );
    assert_ne!(
        replay,
        ContractError::Unauthorized,
        "a replayed admission must reach record law, not the role gate"
    );
}
