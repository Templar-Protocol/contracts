//! Vault CLI - Full 1:1 command-line interface for the Templar vault client.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use near_primitives::types::Gas;
use templar_common::vault::SUPPLY_GAS;
use templar_vault_client::{
    AccountId, AllocationDelta, CapGroupUpdate, CapGroupUpdateKey, ErrorWrapper, Fees,
    KeyCredential, KeyPoolClient, KeyPoolConfig, MarketId, Restrictions, TimelockKind, VaultClient,
    VaultViewClient,
};

const fn tgas(t: u64) -> Gas {
    t * 1_000_000_000_000
}

#[derive(Parser)]
#[command(name = "vault-cli")]
#[command(about = "Command-line interface for the Templar vault client")]
#[command(version)]
struct Cli {
    #[command(flatten)]
    global: GlobalOpts,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Args)]
struct GlobalOpts {
    #[arg(
        long,
        env = "NEAR_RPC_URL",
        default_value = "https://rpc.mainnet.near.org"
    )]
    rpc_url: String,

    #[arg(long, env = "VAULT_ACCOUNT")]
    vault: String,

    #[arg(long, env = "SIGNER_ACCOUNT_ID")]
    signer_account_id: Option<String>,

    #[arg(long, env = "SIGNER_SECRET_KEY")]
    signer_secret_key: Option<String>,

    #[arg(long, default_value = "vault")]
    client: ClientType,

    #[arg(long, default_value = "json")]
    output: OutputFormat,
}

#[derive(Clone, Copy, ValueEnum)]
enum ClientType {
    Vault,
    Keypool,
}

#[derive(Clone, Copy, ValueEnum)]
enum OutputFormat {
    Json,
    Pretty,
}

#[derive(Subcommand)]
enum Commands {
    #[command(subcommand)]
    View(ViewCommands),

    #[command(subcommand)]
    Tx(TxCommands),
}

#[derive(Subcommand)]
enum ViewCommands {
    GetConfiguration,
    GetTotalAssets,
    GetLastTotalAssets,
    GetIdleBalance,
    GetTotalSupply,
    GetMaxDeposit,
    GetMaxSingleMarketDeposit,
    GetFeeAnchor,
    GetFees,
    GetRestrictions,
    GetCapGroups,
    GetPendingGovernanceActions,
    GetWithdrawingOpId,
    HasPendingMarketWithdrawal,
    GetCurrentWithdrawRequestId,
    QueueTail,
    PeekNextPendingWithdrawalId,
    BuildRealAssetsReport,
    ListMarketsWithIds,
    GetVaultSnapshot,
    ConvertToShares(ConvertArgs),
    ConvertToAssets(ConvertArgs),
    PreviewDeposit(ConvertArgs),
    PreviewMint(ConvertArgs),
    PreviewWithdraw(ConvertArgs),
    PreviewRedeem(ConvertArgs),
    GetMarketIdOfAccount(MarketAccountArgs),
    GetMarketAccountById(MarketIdArgs),
    ResolveMarketIds(ResolveMarketIdsArgs),
    ResolveMarketAccounts(ResolveMarketAccountsArgs),
}

#[derive(Args)]
struct ConvertArgs {
    #[arg(long)]
    amount: String,
}

#[derive(Args)]
struct MarketAccountArgs {
    #[arg(long)]
    market: String,
}

#[derive(Args)]
struct MarketIdArgs {
    #[arg(long)]
    market_id: u32,
}

#[derive(Args)]
struct ResolveMarketIdsArgs {
    #[arg(long, value_delimiter = ',')]
    markets: Vec<String>,
}

#[derive(Args)]
struct ResolveMarketAccountsArgs {
    #[arg(long, value_delimiter = ',')]
    market_ids: Vec<u32>,
}

#[derive(Subcommand)]
enum TxCommands {
    DepositSupply(DepositSupplyArgs),
    Withdraw(WithdrawTxArgs),
    Redeem(RedeemTxArgs),
    Reallocate(ReallocateArgs),
    ExecuteWithdrawal(ExecuteWithdrawalArgs),
    ExecuteMarketWithdrawal(ExecuteMarketWithdrawalArgs),
    ExecuteRebalanceWithdrawal(ExecuteRebalanceWithdrawalArgs),
    RefreshMarkets(RefreshMarketsArgs),
    RefreshAllMarkets,
    SetSupplyQueue(SetSupplyQueueArgs),
    SetCurator(AccountArg),
    SetIsAllocator(SetIsAllocatorArgs),
    SubmitGuardian(AccountArg),
    AcceptGuardian,
    RevokePendingGuardian,
    SubmitSentinel(AccountArg),
    AcceptSentinel,
    RevokePendingSentinel,
    SetSkimRecipient(AccountArg),
    Skim(AccountArg),
    SetFees(JsonArg),
    AcceptFees,
    RevokePendingFees,
    SubmitTimelock(SubmitTimelockArgs),
    AcceptTimelock,
    RevokePendingTimelock,
    SubmitCap(SubmitCapArgs),
    AcceptCap(AccountArg),
    RevokePendingCap(AccountArg),
    SubmitCapGroupUpdate(JsonArg),
    AcceptCapGroupUpdate(JsonArg),
    RevokePendingCapGroupUpdate(JsonArg),
    SetRestrictions(JsonArg),
    AcceptRestrictions,
    RevokePendingRestrictions,
    SubmitMarketRemoval(AccountArg),
    AcceptMarketRemoval(AccountArg),
    RevokePendingMarketRemoval(AccountArg),
    Unbrick,
    Abdicate(AbdicateArgs),
    ClearViewCache,
}

#[derive(Args)]
struct DepositSupplyArgs {
    #[arg(long)]
    amount: String,
    #[arg(long)]
    gas_tgas: Option<u64>,
}

#[derive(Args)]
struct WithdrawTxArgs {
    #[arg(long)]
    assets: String,
    #[arg(long)]
    receiver: String,
    #[arg(long, default_value = "1")]
    deposit_yocto: String,
}

#[derive(Args)]
struct RedeemTxArgs {
    #[arg(long)]
    shares: String,
    #[arg(long)]
    receiver: String,
    #[arg(long, default_value = "1")]
    deposit_yocto: String,
}

#[derive(Args)]
struct ReallocateArgs {
    #[arg(long)]
    json: String,
}

#[derive(Args)]
struct ExecuteWithdrawalArgs {
    #[arg(long, value_delimiter = ',')]
    route: Vec<u32>,
}

#[derive(Args)]
struct ExecuteMarketWithdrawalArgs {
    #[arg(long)]
    op_id: u64,
    #[arg(long)]
    market_id: u32,
    #[arg(long)]
    batch_limit: Option<u32>,
}

#[derive(Args)]
struct ExecuteRebalanceWithdrawalArgs {
    #[arg(long)]
    market_id: u32,
    #[arg(long)]
    batch_limit: Option<u32>,
}

#[derive(Args)]
struct RefreshMarketsArgs {
    #[arg(long, value_delimiter = ',')]
    market_ids: Vec<u32>,
}

#[derive(Args)]
struct SetSupplyQueueArgs {
    #[arg(long, value_delimiter = ',')]
    market_ids: Vec<u32>,
    #[arg(long, default_value = "1")]
    deposit_yocto: String,
}

#[derive(Args)]
struct AccountArg {
    #[arg(long)]
    account: String,
}

#[derive(Args)]
struct SetIsAllocatorArgs {
    #[arg(long)]
    account: String,
    #[arg(long)]
    allowed: bool,
}

#[derive(Args)]
struct JsonArg {
    #[arg(long)]
    json: String,
}

#[derive(Args)]
struct SubmitTimelockArgs {
    #[arg(long)]
    new_timelock_ns: u64,
    #[arg(long)]
    kind: Option<String>,
}

#[derive(Args)]
struct SubmitCapArgs {
    #[arg(long)]
    market: String,
    #[arg(long)]
    new_cap: String,
}

#[derive(Args)]
struct AbdicateArgs {
    #[arg(long)]
    method_name: String,
}

enum Client {
    Vault(VaultClient),
    KeyPool(KeyPoolClient),
    View(VaultViewClient),
}

impl Client {
    fn new_view(opts: &GlobalOpts) -> Result<Self, ErrorWrapper> {
        let vault = AccountId::from(opts.vault.clone());
        let client = VaultViewClient::new_default(opts.rpc_url.clone(), &vault)?;
        Ok(Client::View(client))
    }

    fn new_tx(opts: &GlobalOpts) -> Result<Self, ErrorWrapper> {
        let vault = AccountId::from(opts.vault.clone());

        let signer_account_id = opts.signer_account_id.clone().ok_or_else(|| {
            ErrorWrapper::Wrapped("Missing --signer-account-id (or SIGNER_ACCOUNT_ID)".to_string())
        })?;
        let signer_secret_key = opts.signer_secret_key.clone().ok_or_else(|| {
            ErrorWrapper::Wrapped("Missing --signer-secret-key (or SIGNER_SECRET_KEY)".to_string())
        })?;

        let credential = KeyCredential {
            account_id: AccountId::from(signer_account_id),
            secret_key: signer_secret_key,
        };

        match opts.client {
            ClientType::Vault => {
                let client =
                    VaultClient::new_single_key_default(opts.rpc_url.clone(), &vault, credential)?;
                Ok(Client::Vault(client))
            }
            ClientType::Keypool => {
                let client = KeyPoolClient::new(
                    opts.rpc_url.clone(),
                    &vault,
                    vec![credential],
                    KeyPoolConfig::default(),
                )?;
                Ok(Client::KeyPool(client))
            }
        }
    }
}

fn output_json<T: serde::Serialize>(format: OutputFormat, value: &T) {
    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
            );
        }
        OutputFormat::Pretty => {
            println!(
                "{}",
                serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".to_string())
            );
        }
    }
}

#[derive(serde::Serialize)]
struct OkResponse {
    ok: bool,
}

#[derive(serde::Serialize)]
struct ErrorResponse<'a> {
    error: &'a str,
}

fn output_ok(format: OutputFormat) {
    output_json(format, &OkResponse { ok: true });
}

fn output_error(format: OutputFormat, error: &str) {
    output_json(format, &ErrorResponse { error });
}

fn parse_json_arg<T: serde::de::DeserializeOwned>(json: &str) -> Result<T, String> {
    let content = if json.starts_with('@') {
        let path = PathBuf::from(&json[1..]);
        std::fs::read_to_string(&path).map_err(|e| format!("Failed to read file: {}", e))?
    } else {
        json.to_string()
    };
    serde_json::from_str(&content).map_err(|e| format!("Failed to parse JSON: {}", e))
}

fn parse_timelock_kind(s: &str) -> Option<TimelockKind> {
    match s.to_lowercase().as_str() {
        "guardian" => Some(TimelockKind::Guardian),
        "sentinel" => Some(TimelockKind::Sentinel),
        "config" => Some(TimelockKind::Config),
        "cap" => Some(TimelockKind::Cap),
        "marketremoval" | "market_removal" | "market-removal" => Some(TimelockKind::MarketRemoval),
        _ => None,
    }
}

macro_rules! dispatch_view {
    ($client:expr, $method:ident) => {
        match $client {
            Client::Vault(c) => c.$method().await,
            Client::KeyPool(c) => c.$method().await,
            Client::View(c) => c.$method().await,
        }
    };
    ($client:expr, $method:ident, $($arg:expr),+) => {
        match $client {
            Client::Vault(c) => c.$method($($arg),+).await,
            Client::KeyPool(c) => c.$method($($arg),+).await,
            Client::View(c) => c.$method($($arg),+).await,
        }
    };
}

macro_rules! view_json {
    ($format:expr, $res:expr) => {
        match $res {
            Ok(v) => output_json($format, &v),
            Err(e) => output_error($format, &format!("{:?}", e)),
        }
    };
}

macro_rules! view_json_map {
    ($format:expr, $res:expr, $map:expr) => {
        match $res {
            Ok(v) => output_json($format, &($map)(v)),
            Err(e) => output_error($format, &format!("{:?}", e)),
        }
    };
}

async fn handle_view(client: &Client, cmd: ViewCommands, format: OutputFormat) {
    match cmd {
        ViewCommands::GetConfiguration => {
            view_json!(format, dispatch_view!(client, get_configuration))
        }
        ViewCommands::GetTotalAssets => {
            view_json!(format, dispatch_view!(client, get_total_assets))
        }
        ViewCommands::GetLastTotalAssets => {
            view_json!(format, dispatch_view!(client, get_last_total_assets))
        }
        ViewCommands::GetIdleBalance => {
            view_json!(format, dispatch_view!(client, get_idle_balance))
        }
        ViewCommands::GetTotalSupply => {
            view_json!(format, dispatch_view!(client, get_total_supply))
        }
        ViewCommands::GetMaxDeposit => view_json!(format, dispatch_view!(client, get_max_deposit)),
        ViewCommands::GetMaxSingleMarketDeposit => {
            view_json!(
                format,
                dispatch_view!(client, get_max_single_market_deposit)
            )
        }
        ViewCommands::GetFeeAnchor => view_json!(format, dispatch_view!(client, get_fee_anchor)),
        ViewCommands::GetFees => view_json!(format, dispatch_view!(client, get_fees)),
        ViewCommands::GetRestrictions => {
            view_json!(format, dispatch_view!(client, get_restrictions))
        }
        ViewCommands::GetCapGroups => view_json!(format, dispatch_view!(client, get_cap_groups)),
        ViewCommands::GetPendingGovernanceActions => {
            view_json!(
                format,
                dispatch_view!(client, get_pending_governance_actions)
            )
        }
        ViewCommands::GetWithdrawingOpId => {
            view_json!(format, dispatch_view!(client, get_withdrawing_op_id))
        }
        ViewCommands::HasPendingMarketWithdrawal => {
            view_json!(
                format,
                dispatch_view!(client, has_pending_market_withdrawal)
            )
        }
        ViewCommands::GetCurrentWithdrawRequestId => {
            view_json!(
                format,
                dispatch_view!(client, get_current_withdraw_request_id)
            )
        }
        ViewCommands::QueueTail => view_json!(format, dispatch_view!(client, queue_tail)),
        ViewCommands::PeekNextPendingWithdrawalId => {
            view_json!(
                format,
                dispatch_view!(client, peek_next_pending_withdrawal_id)
            )
        }
        ViewCommands::BuildRealAssetsReport => {
            view_json!(format, dispatch_view!(client, build_real_assets_report))
        }
        ViewCommands::ListMarketsWithIds => {
            view_json!(format, dispatch_view!(client, list_markets_with_ids))
        }
        ViewCommands::GetVaultSnapshot => {
            view_json!(format, dispatch_view!(client, get_vault_snapshot))
        }
        ViewCommands::ConvertToShares(args) => {
            view_json!(
                format,
                dispatch_view!(client, convert_to_shares, &args.amount)
            )
        }
        ViewCommands::ConvertToAssets(args) => {
            view_json!(
                format,
                dispatch_view!(client, convert_to_assets, &args.amount)
            )
        }
        ViewCommands::PreviewDeposit(args) => {
            view_json!(
                format,
                dispatch_view!(client, preview_deposit, &args.amount)
            )
        }
        ViewCommands::PreviewMint(args) => {
            view_json!(format, dispatch_view!(client, preview_mint, &args.amount))
        }
        ViewCommands::PreviewWithdraw(args) => {
            view_json!(
                format,
                dispatch_view!(client, preview_withdraw, &args.amount)
            )
        }
        ViewCommands::PreviewRedeem(args) => {
            view_json!(format, dispatch_view!(client, preview_redeem, &args.amount))
        }
        ViewCommands::GetMarketIdOfAccount(args) => {
            let market = AccountId::from(args.market);
            view_json_map!(
                format,
                dispatch_view!(client, get_market_id_of_account, &market),
                |v: Option<MarketId>| v.map(|m| m.0)
            )
        }
        ViewCommands::GetMarketAccountById(args) => {
            let market_id = MarketId::from(args.market_id);
            view_json_map!(
                format,
                dispatch_view!(client, get_market_account_by_id, market_id),
                |v: Option<AccountId>| v.map(String::from)
            )
        }
        ViewCommands::ResolveMarketIds(args) => {
            let markets: Vec<AccountId> = args.markets.into_iter().map(AccountId::from).collect();
            view_json_map!(
                format,
                dispatch_view!(client, resolve_market_ids, &markets),
                |v: Vec<Option<MarketId>>| v.iter().map(|o| o.map(|m| m.0)).collect::<Vec<_>>()
            )
        }
        ViewCommands::ResolveMarketAccounts(args) => {
            let market_ids: Vec<MarketId> =
                args.market_ids.into_iter().map(MarketId::from).collect();
            view_json_map!(
                format,
                dispatch_view!(client, resolve_market_accounts, &market_ids),
                |v: Vec<Option<AccountId>>| {
                    v.iter()
                        .map(|o| o.as_ref().map(|a| String::from(a.clone())))
                        .collect::<Vec<_>>()
                }
            )
        }
    }
}

macro_rules! dispatch_tx {
    ($client:expr, $method:ident) => {
        match $client {
            Client::Vault(c) => c.$method().await,
            Client::KeyPool(c) => c.$method().await,
            Client::View(_) => Err(ErrorWrapper::Wrapped(
                "VaultViewClient is read-only; tx commands are not supported".to_string(),
            )),
        }
    };
    ($client:expr, $method:ident, $($arg:expr),+) => {
        match $client {
            Client::Vault(c) => c.$method($($arg),+).await,
            Client::KeyPool(c) => c.$method($($arg),+).await,
            Client::View(_) => Err(ErrorWrapper::Wrapped(
                "VaultViewClient is read-only; tx commands are not supported".to_string(),
            )),
        }
    };
}

macro_rules! tx_ok {
    ($format:expr, $res:expr) => {
        match $res {
            Ok(()) => output_ok($format),
            Err(e) => output_error($format, &format!("{:?}", e)),
        }
    };
}

macro_rules! tx_json {
    ($format:expr, $res:expr) => {
        match $res {
            Ok(v) => output_json($format, &v),
            Err(e) => output_error($format, &format!("{:?}", e)),
        }
    };
}

async fn handle_tx(client: &Client, cmd: TxCommands, format: OutputFormat) {
    match cmd {
        TxCommands::DepositSupply(args) => {
            let gas = args.gas_tgas.map(tgas).or(Some(SUPPLY_GAS.as_gas()));
            tx_ok!(
                format,
                dispatch_tx!(client, deposit_supply, &args.amount, gas)
            )
        }
        TxCommands::Withdraw(args) => {
            let receiver = AccountId::from(args.receiver);
            tx_ok!(
                format,
                dispatch_tx!(
                    client,
                    withdraw,
                    &args.assets,
                    &receiver,
                    &args.deposit_yocto
                )
            )
        }
        TxCommands::Redeem(args) => {
            let receiver = AccountId::from(args.receiver);
            tx_ok!(
                format,
                dispatch_tx!(client, redeem, &args.shares, &receiver, &args.deposit_yocto)
            )
        }
        TxCommands::Reallocate(args) => {
            let common_delta: templar_common::vault::AllocationDelta =
                match parse_json_arg(&args.json) {
                    Ok(d) => d,
                    Err(e) => {
                        output_error(format, &e);
                        return;
                    }
                };
            let delta: AllocationDelta = common_delta.into();
            tx_ok!(format, dispatch_tx!(client, reallocate, &delta))
        }
        TxCommands::ExecuteWithdrawal(args) => {
            let route: Vec<MarketId> = args.route.into_iter().map(MarketId::from).collect();
            tx_ok!(format, dispatch_tx!(client, execute_withdrawal, &route))
        }
        TxCommands::ExecuteMarketWithdrawal(args) => {
            let market = MarketId::from(args.market_id);
            tx_ok!(
                format,
                dispatch_tx!(
                    client,
                    execute_market_withdrawal,
                    args.op_id,
                    market,
                    args.batch_limit
                )
            )
        }
        TxCommands::ExecuteRebalanceWithdrawal(args) => {
            let market = MarketId::from(args.market_id);
            tx_ok!(
                format,
                dispatch_tx!(
                    client,
                    execute_rebalance_withdrawal,
                    market,
                    args.batch_limit
                )
            )
        }
        TxCommands::RefreshMarkets(args) => {
            let market_ids: Vec<MarketId> =
                args.market_ids.into_iter().map(MarketId::from).collect();
            tx_json!(format, dispatch_tx!(client, refresh_markets, &market_ids))
        }
        TxCommands::RefreshAllMarkets => {
            tx_json!(format, dispatch_tx!(client, refresh_all_markets))
        }
        TxCommands::SetSupplyQueue(args) => {
            let market_ids: Vec<MarketId> =
                args.market_ids.into_iter().map(MarketId::from).collect();
            tx_ok!(
                format,
                dispatch_tx!(client, set_supply_queue, &market_ids, &args.deposit_yocto)
            )
        }
        TxCommands::SetCurator(args) => {
            let account = AccountId::from(args.account);
            tx_ok!(format, dispatch_tx!(client, set_curator, &account))
        }
        TxCommands::SetIsAllocator(args) => {
            let account = AccountId::from(args.account);
            tx_ok!(
                format,
                dispatch_tx!(client, set_is_allocator, &account, args.allowed)
            )
        }
        TxCommands::SubmitGuardian(args) => {
            let account = AccountId::from(args.account);
            tx_ok!(format, dispatch_tx!(client, submit_guardian, &account))
        }
        TxCommands::AcceptGuardian => tx_ok!(format, dispatch_tx!(client, accept_guardian)),
        TxCommands::RevokePendingGuardian => {
            tx_ok!(format, dispatch_tx!(client, revoke_pending_guardian))
        }
        TxCommands::SubmitSentinel(args) => {
            let account = AccountId::from(args.account);
            tx_ok!(format, dispatch_tx!(client, submit_sentinel, &account))
        }
        TxCommands::AcceptSentinel => tx_ok!(format, dispatch_tx!(client, accept_sentinel)),
        TxCommands::RevokePendingSentinel => {
            tx_ok!(format, dispatch_tx!(client, revoke_pending_sentinel))
        }
        TxCommands::SetSkimRecipient(args) => {
            let account = AccountId::from(args.account);
            tx_ok!(format, dispatch_tx!(client, set_skim_recipient, &account))
        }
        TxCommands::Skim(args) => {
            let account = AccountId::from(args.account);
            tx_ok!(format, dispatch_tx!(client, skim, &account))
        }
        TxCommands::SetFees(args) => {
            let common_fees: templar_common::vault::Fees<near_sdk::json_types::U128> =
                match parse_json_arg(&args.json) {
                    Ok(f) => f,
                    Err(e) => {
                        output_error(format, &e);
                        return;
                    }
                };
            let fees: Fees = common_fees.into();
            tx_ok!(format, dispatch_tx!(client, set_fees, fees))
        }
        TxCommands::AcceptFees => tx_ok!(format, dispatch_tx!(client, accept_fees)),
        TxCommands::RevokePendingFees => tx_ok!(format, dispatch_tx!(client, revoke_pending_fees)),
        TxCommands::SubmitTimelock(args) => {
            let kind = args.kind.as_ref().and_then(|s| parse_timelock_kind(s));
            tx_ok!(
                format,
                dispatch_tx!(client, submit_timelock, args.new_timelock_ns, kind)
            )
        }
        TxCommands::AcceptTimelock => tx_ok!(format, dispatch_tx!(client, accept_timelock)),
        TxCommands::RevokePendingTimelock => {
            tx_ok!(format, dispatch_tx!(client, revoke_pending_timelock))
        }
        TxCommands::SubmitCap(args) => {
            let market = AccountId::from(args.market);
            tx_ok!(
                format,
                dispatch_tx!(client, submit_cap, &market, &args.new_cap)
            )
        }
        TxCommands::AcceptCap(args) => {
            let market = AccountId::from(args.account);
            tx_ok!(format, dispatch_tx!(client, accept_cap, &market))
        }
        TxCommands::RevokePendingCap(args) => {
            let market = AccountId::from(args.account);
            tx_ok!(format, dispatch_tx!(client, revoke_pending_cap, &market))
        }
        TxCommands::SubmitCapGroupUpdate(args) => {
            let common_update: templar_common::vault::CapGroupUpdate =
                match parse_json_arg(&args.json) {
                    Ok(u) => u,
                    Err(e) => {
                        output_error(format, &e);
                        return;
                    }
                };
            let update: CapGroupUpdate = match common_update {
                templar_common::vault::CapGroupUpdate::SetCap { cap_group, new_cap } => {
                    CapGroupUpdate::SetCap {
                        cap_group: cap_group.0.into(),
                        new_cap: new_cap.0.to_string(),
                    }
                }
                templar_common::vault::CapGroupUpdate::SetRelativeCap {
                    cap_group,
                    new_relative_cap,
                } => CapGroupUpdate::SetRelativeCap {
                    cap_group: cap_group.0.into(),
                    new_relative_cap: new_relative_cap.0.to_string(),
                },
                templar_common::vault::CapGroupUpdate::SetMarketCapGroup { market, cap_group } => {
                    CapGroupUpdate::SetMarketCapGroup {
                        market: market.0.into(),
                        cap_group: cap_group.map(|g| g.0.into()),
                    }
                }
            };
            tx_ok!(
                format,
                dispatch_tx!(client, submit_cap_group_update, update)
            )
        }
        TxCommands::AcceptCapGroupUpdate(args) => {
            let common_key: templar_common::vault::CapGroupUpdateKey =
                match parse_json_arg(&args.json) {
                    Ok(k) => k,
                    Err(e) => {
                        output_error(format, &e);
                        return;
                    }
                };
            let key: CapGroupUpdateKey = match common_key {
                templar_common::vault::CapGroupUpdateKey::SetCap { cap_group } => {
                    CapGroupUpdateKey::SetCap {
                        cap_group: cap_group.0.into(),
                    }
                }
                templar_common::vault::CapGroupUpdateKey::SetRelativeCap { cap_group } => {
                    CapGroupUpdateKey::SetRelativeCap {
                        cap_group: cap_group.0.into(),
                    }
                }
                templar_common::vault::CapGroupUpdateKey::SetMarketCapGroup { market } => {
                    CapGroupUpdateKey::SetMarketCapGroup {
                        market: market.0.into(),
                    }
                }
            };
            tx_ok!(format, dispatch_tx!(client, accept_cap_group_update, key))
        }
        TxCommands::RevokePendingCapGroupUpdate(args) => {
            let common_key: templar_common::vault::CapGroupUpdateKey =
                match parse_json_arg(&args.json) {
                    Ok(k) => k,
                    Err(e) => {
                        output_error(format, &e);
                        return;
                    }
                };
            let key: CapGroupUpdateKey = match common_key {
                templar_common::vault::CapGroupUpdateKey::SetCap { cap_group } => {
                    CapGroupUpdateKey::SetCap {
                        cap_group: cap_group.0.into(),
                    }
                }
                templar_common::vault::CapGroupUpdateKey::SetRelativeCap { cap_group } => {
                    CapGroupUpdateKey::SetRelativeCap {
                        cap_group: cap_group.0.into(),
                    }
                }
                templar_common::vault::CapGroupUpdateKey::SetMarketCapGroup { market } => {
                    CapGroupUpdateKey::SetMarketCapGroup {
                        market: market.0.into(),
                    }
                }
            };
            tx_ok!(
                format,
                dispatch_tx!(client, revoke_pending_cap_group_update, key)
            )
        }
        TxCommands::SetRestrictions(args) => {
            let common_restrictions: Option<templar_common::vault::Restrictions> =
                match parse_json_arg(&args.json) {
                    Ok(r) => r,
                    Err(e) => {
                        output_error(format, &e);
                        return;
                    }
                };
            let restrictions: Option<Restrictions> = common_restrictions.map(Into::into);
            tx_ok!(format, dispatch_tx!(client, set_restrictions, restrictions))
        }
        TxCommands::AcceptRestrictions => tx_ok!(format, dispatch_tx!(client, accept_restrictions)),
        TxCommands::RevokePendingRestrictions => {
            tx_ok!(format, dispatch_tx!(client, revoke_pending_restrictions))
        }
        TxCommands::SubmitMarketRemoval(args) => {
            let market = AccountId::from(args.account);
            tx_ok!(format, dispatch_tx!(client, submit_market_removal, &market))
        }
        TxCommands::AcceptMarketRemoval(args) => {
            let market = AccountId::from(args.account);
            tx_ok!(format, dispatch_tx!(client, accept_market_removal, &market))
        }
        TxCommands::RevokePendingMarketRemoval(args) => {
            let market = AccountId::from(args.account);
            tx_ok!(
                format,
                dispatch_tx!(client, revoke_pending_market_removal, &market)
            )
        }
        TxCommands::Unbrick => tx_ok!(format, dispatch_tx!(client, unbrick)),
        TxCommands::Abdicate(args) => {
            tx_ok!(format, dispatch_tx!(client, abdicate, args.method_name))
        }
        TxCommands::ClearViewCache => tx_ok!(format, dispatch_tx!(client, clear_view_cache)),
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    let format = cli.global.output;

    match cli.command {
        Commands::View(cmd) => {
            let client = match Client::new_view(&cli.global) {
                Ok(c) => c,
                Err(e) => {
                    output_error(format, &format!("Failed to create client: {:?}", e));
                    std::process::exit(1);
                }
            };
            handle_view(&client, cmd, format).await
        }
        Commands::Tx(cmd) => {
            let client = match Client::new_tx(&cli.global) {
                Ok(c) => c,
                Err(e) => {
                    output_error(format, &format!("Failed to create client: {:?}", e));
                    std::process::exit(1);
                }
            };
            handle_tx(&client, cmd, format).await
        }
    }
}
