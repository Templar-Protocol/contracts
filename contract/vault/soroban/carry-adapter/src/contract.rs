//! CarryAdapterContract: the Templar vault market adapter for a Precept
//! (Blend v2 fork) carry trade.
//!
//! Implements the frozen vault market ABI (`supply`, `progress_withdrawal`,
//! `total_assets`) plus the admin/view surface specified in the carry plan
//! §4. Position accounting delegates to [`crate::math`]; pool interaction
//! goes through [`crate::pool_ops`].

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address,
    BytesN, Env, Vec,
};
use stellar_contract_utils::upgradeable::Upgradeable;
use templar_soroban_adapter_common::{
    accept_admin as common_accept_admin, admin as common_admin, extend_instance_ttl,
    init_admin_vault, is_paused, propose_admin, require_admin, require_admin_or_vault,
    require_contract_address, require_not_paused, require_vault, set_paused_flag, upgrade_as_admin,
    CommonError,
};

use crate::math::{self, ConvCtx};
use crate::oracle_client::{Asset, PriceData, PriceFeedClient};
use crate::pool_ops::Request;
use crate::pool_ops::{
    reserve_liabilities, reserve_supply, PoolOps, Reserve, REQUEST_BORROW,
    REQUEST_DELETE_LIQ_AUCTION, REQUEST_REPAY, REQUEST_SUPPLY, REQUEST_SUPPLY_COLLATERAL,
    REQUEST_WITHDRAW, REQUEST_WITHDRAW_COLLATERAL,
};

#[contracttype]
#[derive(Clone, Copy)]
pub struct Params {
    pub borrow_utilization_bps: u32,
    pub surplus_buffer_bps: u32,
    pub surplus_haircut_bps: u32,
    pub oracle_free_band_bps: u32,
    pub oracle_max_age_secs: u64,
    pub min_residual_collateral: i128,
}

#[contracttype]
#[derive(Clone, Copy)]
pub enum DataKey {
    Pool,
    YieldPool,
    CollateralAsset,
    BorrowAsset,
    SurplusRecipient,
    Params,
    RealisationPending,
}

#[contracttype]
#[derive(Clone, Copy)]
pub struct PositionView {
    /// Collateral underlying held in `pool` (c units).
    pub c_pool: i128,
    /// Idle collateral held by the adapter (c units).
    pub idle_c: i128,
    /// Total collateral `C = c_pool + idle_c`.
    pub c: i128,
    /// Debt `D` in b units (rounded up).
    pub d: i128,
    /// Idle borrow asset held by the adapter (b units).
    pub idle_b: i128,
    /// Yield leg underlying in `yield_pool` (b units).
    pub y_pool: i128,
    /// Total yield leg `Y = y_pool + idle_b`.
    pub y: i128,
}

#[contracterror]
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum CarryError {
    Unauthorized = 1,
    InvalidInput = 2,
    MissingConfig = 3,
    ArithmeticOverflow = 4,
    ArithmeticUnderflow = 5,
    ZeroWithdrawal = 6,
    Paused = 7,
    InvalidOracleDecimals = 9,
    OracleStale = 10,
    RealisationPending = 11,
    InvalidParams = 12,
    NothingToSweep = 13,
    InvalidAsset = 14,
}

impl From<math::MathError> for CarryError {
    fn from(_err: math::MathError) -> CarryError {
        CarryError::InvalidParams
    }
}

impl From<CommonError> for CarryError {
    fn from(err: CommonError) -> CarryError {
        match err {
            CommonError::Unauthorized => CarryError::Unauthorized,
            CommonError::MissingConfig => CarryError::MissingConfig,
            CommonError::Paused => CarryError::Paused,
            // There is no NotContract variant; constructor validation mapped
            // non-contract addresses to InvalidInput.
            CommonError::NotContract => CarryError::InvalidInput,
        }
    }
}

#[contract]
pub struct CarryAdapterContract;

#[contractimpl]
impl CarryAdapterContract {
    /// Runs atomically during deployment; no separate initialize call.
    #[allow(clippy::too_many_arguments)]
    pub fn __constructor(
        env: Env,
        admin: Address,
        vault: Address,
        pool: Address,
        yield_pool: Address,
        collateral_asset: Address,
        borrow_asset: Address,
        surplus_recipient: Address,
        borrow_utilization_bps: u32,
        surplus_buffer_bps: u32,
        surplus_haircut_bps: u32,
        oracle_free_band_bps: u32,
        oracle_max_age_secs: u64,
        min_residual_collateral: i128,
    ) -> Result<(), CarryError> {
        extend_instance_ttl(&env);
        require_contract_address(&vault)?;
        require_contract_address(&pool)?;
        require_contract_address(&yield_pool)?;
        if pool == yield_pool || collateral_asset == borrow_asset {
            return Err(CarryError::InvalidInput);
        }
        let params = Params {
            borrow_utilization_bps,
            surplus_buffer_bps,
            surplus_haircut_bps,
            oracle_free_band_bps,
            oracle_max_age_secs,
            min_residual_collateral,
        };
        params.validate()?;

        init_admin_vault(&env, &admin, &vault);
        let storage = env.storage().instance();
        storage.set(&DataKey::Pool, &pool);
        storage.set(&DataKey::YieldPool, &yield_pool);
        storage.set(&DataKey::CollateralAsset, &collateral_asset);
        storage.set(&DataKey::BorrowAsset, &borrow_asset);
        storage.set(&DataKey::SurplusRecipient, &surplus_recipient);
        storage.set(&DataKey::Params, &params);
        storage.set(&DataKey::RealisationPending, &false);
        Ok(())
    }

    // ------------------------------------------------------------------
    // Vault market ABI
    // ------------------------------------------------------------------

    /// Supply collateral to `pool` and (re)balance the borrow leg against the
    /// target, deploying borrowed funds into the yield leg.
    #[allow(deprecated)]
    pub fn supply(
        env: Env,
        caller: Address,
        asset: Address,
        amount: i128,
    ) -> Result<(), CarryError> {
        extend_instance_ttl(&env);
        require_vault(&env, &caller)?;
        require_not_paused(&env)?;
        if is_realisation_pending(&env) {
            return Err(CarryError::RealisationPending);
        }
        if amount <= 0 {
            return Err(CarryError::InvalidInput);
        }
        if asset != get_collateral_asset(&env)? {
            return Err(CarryError::InvalidAsset);
        }
        let pool = get_pool(&env)?;
        let borrow_asset = get_borrow_asset(&env)?;
        let adapter = env.current_contract_address();
        let params = get_params(&env)?;

        // A collateral `amount` whose b-token mint rounds to zero is invalid
        // input: the pool would panic on `SupplyCollateral` (#1216). Check
        // this before any submit so nothing executes on a doomed call.
        let c_b_rate = env.reserve(&pool, &asset).data.b_rate;
        if !math::mints_b_tokens(amount, c_b_rate)? {
            return Err(CarryError::InvalidInput);
        }

        // 1. Supply collateral.
        let mut requests = Vec::new(&env);
        requests.push_back(Request {
            request_type: REQUEST_SUPPLY_COLLATERAL,
            address: asset.clone(),
            amount,
        });
        env.submit_auth(&pool, &adapter, &requests, Some((&asset, &adapter, amount)));
        env.events()
            .publish((symbol_short!("supply"), asset.clone()), amount);

        // 2. Borrow to `b_target` on POOL collateral only. Only a stale/absent
        // oracle price skips the borrow; every other failure reverts.
        let s = read_position(&env, &adapter, &pool, &borrow_asset)?;
        let b_target = match target_borrow_b(&env, &s, &asset, &borrow_asset, &params) {
            Ok(target) => target,
            Err(CarryError::OracleStale) => {
                env.events()
                    .publish((symbol_short!("borrow"), symbol_short!("skip")), amount);
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let headroom = math::max_util_headroom(
            reserve_supply(&s.b_reserve),
            reserve_liabilities(&s.b_reserve),
            s.b_reserve.config.max_util,
            s.b_reserve.data.d_rate,
        )?;
        let b = math::borrow_amount(b_target, s.d, headroom)?;
        // `b <= 0` or a zero b-token mint on the yield pool both mean the
        // borrow leg is skipped (#1216); the collateral leg stands on its own.
        if b <= 0 || !math::mints_b_tokens(b, s.y_reserve.data.b_rate)? {
            env.events()
                .publish((symbol_short!("borrow"), symbol_short!("skip")), amount);
            return Ok(());
        }

        // 3. Borrow into the adapter and deposit into the yield leg.
        let mut requests = Vec::new(&env);
        requests.push_back(Request {
            request_type: REQUEST_BORROW,
            address: borrow_asset.clone(),
            amount: b,
        });
        env.submit_auth(
            &pool, &adapter, &requests,
            // The pool transfers borrow funds to the adapter (to=adapter);
            // the pool itself authorizes the token transfer.
            None,
        );
        let yield_pool = get_yield_pool(&env)?;
        let mut requests = Vec::new(&env);
        requests.push_back(Request {
            request_type: REQUEST_SUPPLY,
            address: borrow_asset.clone(),
            amount: b,
        });
        env.submit_auth(
            &yield_pool,
            &adapter,
            &requests,
            Some((&borrow_asset, &adapter, b)),
        );
        env.events()
            .publish((symbol_short!("borrow"), borrow_asset), b);
        Ok(())
    }

    /// Progress a withdrawal: pro-rata pro-debt unwind, idle-first delivery,
    /// returning the collateral actually delivered to the vault.
    #[allow(deprecated)]
    pub fn progress_withdrawal(
        env: Env,
        caller: Address,
        asset: Address,
        amount: i128,
    ) -> Result<i128, CarryError> {
        extend_instance_ttl(&env);
        require_vault(&env, &caller)?;
        if amount <= 0 {
            return Err(CarryError::InvalidInput);
        }
        if asset != get_collateral_asset(&env)? {
            return Err(CarryError::InvalidAsset);
        }
        let pool = get_pool(&env)?;
        let borrow_asset = get_borrow_asset(&env)?;
        let yield_pool = get_yield_pool(&env)?;
        let adapter = env.current_contract_address();
        let params = get_params(&env)?;

        let s = read_position(&env, &adapter, &pool, &borrow_asset)?;
        let (c_pool, idle_c, c, d, idle_b) = (s.c_pool, s.idle_c, s.c(), s.d, s.idle_b);

        // Snapshot the adapter's collateral balance BEFORE the pool batch
        // credits x_pool, so the delivered delta can be measured exactly.
        let vault = templar_soroban_adapter_common::vault(&env)?;
        let c_token = soroban_sdk::token::Client::new(&env, &asset);
        let c_before = c_token.balance(&adapter);

        // Funding sources for the repayment, then the plan.
        let yield_cap = withdrawable(&s.y_reserve, s.y_pool)?;
        let pool_cap = withdrawable(&s.c_reserve, c_pool)?;
        let plan = math::unwind_split(&math::UnwindIn {
            amount,
            c,
            c_pool,
            idle_c,
            d,
            idle_b,
            yield_withdrawable: yield_cap,
            pool_cap,
            min_residual: params.min_residual_collateral,
            min_repay: math::min_repay(s.b_reserve.data.d_rate)?,
        })?;
        if plan.x_pool + plan.x_idle == 0 {
            return Err(CarryError::ZeroWithdrawal);
        }

        // Fund the repayment: idle b first, then the yield leg.
        let idle_part = plan.r.min(idle_b);
        let yield_part = plan.r - idle_part;
        let b_before = env.balance(&borrow_asset, &adapter);
        if yield_part > 0 {
            let mut requests = Vec::new(&env);
            requests.push_back(Request {
                request_type: REQUEST_WITHDRAW,
                address: borrow_asset.clone(),
                amount: yield_part,
            });
            env.submit_auth(&yield_pool, &adapter, &requests, None);
        }
        let b_moved = env.balance(&borrow_asset, &adapter) - b_before;
        if b_moved != yield_part {
            return Err(CarryError::InvalidParams);
        }

        // Pool batch: [delete auction?, Repay(r) if d>0 or r>0,
        // WithdrawCollateral(x_pool)].
        if plan.x_pool > 0 || plan.r > 0 {
            let mut requests = Vec::new(&env);
            if env.has_liquidation_auction(&pool, &adapter) {
                requests.push_back(Request {
                    request_type: REQUEST_DELETE_LIQ_AUCTION,
                    address: borrow_asset.clone(),
                    amount: 0,
                });
            }
            if plan.r > 0 {
                requests.push_back(Request {
                    request_type: REQUEST_REPAY,
                    address: borrow_asset.clone(),
                    amount: plan.r,
                });
            }
            if plan.x_pool > 0 {
                requests.push_back(Request {
                    request_type: REQUEST_WITHDRAW_COLLATERAL,
                    address: asset.clone(),
                    amount: plan.x_pool,
                });
            }
            env.submit_auth(
                &pool,
                &adapter,
                &requests,
                // By now the adapter holds the full repay amount (idle b plus
                // the yield-pool withdrawal above); the pool pulls exactly `r`.
                if plan.r > 0 {
                    Some((&borrow_asset, &adapter, plan.r))
                } else {
                    None
                },
            );
        }

        // The pool may clamp the batch, so measure the actual credit instead
        // of assuming x_pool arrived. The measurement MUST happen before the
        // idle transfer, otherwise `pool_credit` would subtract the idle leg.
        let c_after = c_token.balance(&adapter);
        let pool_credit = c_after
            .checked_sub(c_before)
            .ok_or(CarryError::ArithmeticOverflow)?;
        // Deliver collateral idle-first: `x_idle` goes straight to the vault;
        // the pool batch credited `x_pool` to the adapter, which is paid out
        // in one measured transfer.
        if plan.x_idle > 0 {
            c_token.transfer(&adapter, &vault, &plan.x_idle);
        }
        if pool_credit > 0 {
            c_token.transfer(&adapter, &vault, &pool_credit);
        }
        let delivered = plan
            .x_idle
            .checked_add(pool_credit.max(0))
            .ok_or(CarryError::ArithmeticOverflow)?;
        if delivered <= 0 {
            return Err(CarryError::ZeroWithdrawal);
        }
        env.events()
            .publish((symbol_short!("withdraw"), asset), delivered);
        Ok(delivered)
    }

    /// Mark the market NAV: `C + conv(Y - D)` with the oracle-free band.
    #[allow(deprecated)]
    pub fn total_assets(env: Env, asset: Address) -> Result<i128, CarryError> {
        extend_instance_ttl(&env);
        if is_realisation_pending(&env) {
            return Err(CarryError::RealisationPending);
        }
        if asset != get_collateral_asset(&env)? {
            return Err(CarryError::InvalidAsset);
        }
        let pool = get_pool(&env)?;
        let borrow_asset = get_borrow_asset(&env)?;
        let adapter = env.current_contract_address();
        let params = get_params(&env)?;

        let s = read_position(&env, &adapter, &pool, &borrow_asset)?;
        let (c, y, d) = (s.c(), s.y(), s.d);
        if math::nav_in_band(y, d, params.oracle_free_band_bps)? {
            return Ok(c);
        }
        // Priced path.
        let ctx = conv_ctx(&env, &asset, &borrow_asset, &s, &params)?;
        Ok(math::nav_priced(c, y, d, params.surplus_haircut_bps, &ctx)?)
    }

    // ------------------------------------------------------------------
    // Views
    // ------------------------------------------------------------------

    /// The adapter position snapshot.
    pub fn position(env: Env) -> Result<PositionView, CarryError> {
        extend_instance_ttl(&env);
        let pool = get_pool(&env)?;
        let borrow_asset = get_borrow_asset(&env)?;
        let adapter = env.current_contract_address();
        let s = read_position(&env, &adapter, &pool, &borrow_asset)?;
        Ok(PositionView {
            c_pool: s.c_pool,
            idle_c: s.idle_c,
            c: s.c(),
            d: s.d,
            idle_b: s.idle_b,
            y_pool: s.y_pool,
            y: s.y(),
        })
    }

    /// LTV in bps on the pool position:
    /// `D_value * 1e4 / (C_pool_value * c_factor * l_factor)`, both values in
    /// borrow-asset units. Zero with no collateral or no debt.
    pub fn ltv(env: Env) -> Result<i128, CarryError> {
        extend_instance_ttl(&env);
        let pool = get_pool(&env)?;
        let collateral_asset = get_collateral_asset(&env)?;
        let borrow_asset = get_borrow_asset(&env)?;
        let adapter = env.current_contract_address();
        let s = read_position(&env, &adapter, &pool, &borrow_asset)?;
        if s.c_pool <= 0 || s.d <= 0 {
            return Ok(0);
        }
        let params = get_params(&env)?;
        let ctx = conv_ctx(&env, &collateral_asset, &borrow_asset, &s, &params)?;
        let value_b = ctx.c_to_b(s.c_pool, math::Round::Floor)?;
        let num =
            s.d.checked_mul(math::BPS)
                .and_then(|v| v.checked_mul(math::SCALAR_7))
                .and_then(|v| v.checked_mul(math::SCALAR_7))
                .ok_or(CarryError::ArithmeticOverflow)?;
        let den = value_b
            .checked_mul(i128::from(s.c_reserve.config.c_factor))
            .and_then(|v| v.checked_mul(i128::from(s.b_reserve.config.l_factor)))
            .ok_or(CarryError::ArithmeticOverflow)?;
        if den == 0 {
            return Err(CarryError::InvalidParams);
        }
        Ok(num / den)
    }

    pub fn realisation_pending(env: Env) -> bool {
        extend_instance_ttl(&env);
        is_realisation_pending(&env)
    }

    pub fn paused(env: Env) -> bool {
        extend_instance_ttl(&env);
        is_paused(&env)
    }

    // ------------------------------------------------------------------
    // Admin surface
    // ------------------------------------------------------------------
    /// Pause or unpause (admin or vault, as blend-adapter). Pause blocks `supply` only.
    #[allow(deprecated)]
    pub fn set_paused(env: Env, caller: Address, paused: bool) -> Result<(), CarryError> {
        extend_instance_ttl(&env);
        require_admin_or_vault(&env, &caller)?;
        set_paused_flag(&env, &caller, paused);
        Ok(())
    }

    /// Repay debt from idle b and the yield leg without releasing collateral.
    #[allow(deprecated)]
    pub fn deleverage(env: Env, caller: Address) -> Result<(), CarryError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        let pool = get_pool(&env)?;
        let borrow_asset = get_borrow_asset(&env)?;
        let yield_pool = get_yield_pool(&env)?;
        let adapter = env.current_contract_address();

        let s = read_position(&env, &adapter, &pool, &borrow_asset)?;
        let (d, idle_b) = (s.d, s.idle_b);
        if d <= 0 {
            return Ok(());
        }
        let yield_cap = withdrawable(&s.y_reserve, s.y_pool)?;
        let r = d.min(idle_b + yield_cap);
        // `Repay(r)` burns `floor(r * 1e12 / d_rate)` d-tokens and the pool
        // panics on zero (#1219). Raise `r` to the smallest burnable amount
        // when the debt covers it and it is fundable; otherwise deleverage
        // nothing (sub-d-token debt cannot be burned at all).
        let min_repay = math::min_repay(s.b_reserve.data.d_rate)?;
        let mut r = r;
        if r > 0 && r < min_repay {
            if min_repay <= d && idle_b + yield_cap >= min_repay {
                r = min_repay;
            } else {
                return Err(CarryError::ZeroWithdrawal);
            }
        }
        if r <= 0 {
            // No funding source at all: refuse rather than submit Repay(0).
            return Err(CarryError::ZeroWithdrawal);
        }
        let idle_part = r.min(idle_b);
        let yield_part = r - idle_part;
        if yield_part > 0 {
            let mut requests = Vec::new(&env);
            requests.push_back(Request {
                request_type: REQUEST_WITHDRAW,
                address: borrow_asset.clone(),
                amount: yield_part,
            });
            env.submit_auth(&yield_pool, &adapter, &requests, None);
        }
        let mut requests = Vec::new(&env);
        if env.has_liquidation_auction(&pool, &adapter) {
            requests.push_back(Request {
                request_type: REQUEST_DELETE_LIQ_AUCTION,
                address: borrow_asset.clone(),
                amount: 0,
            });
        }
        requests.push_back(Request {
            request_type: REQUEST_REPAY,
            address: borrow_asset.clone(),
            amount: r,
        });
        // The adapter holds exactly `idle_part` of borrow asset pre-batch
        // (the yield withdrawal was transferred into it above).
        env.submit_auth(
            &pool,
            &adapter,
            &requests,
            Some((&borrow_asset, &adapter, r)),
        );
        env.events().publish((symbol_short!("dlevg"), r), r);
        Ok(())
    }

    /// Withdraw the yield-leg surplus above the buffer and pay it out to
    /// `surplus_recipient`, setting `realisation_pending` (fail-closed while
    /// the surplus is in donation transit).
    #[allow(deprecated)]
    pub fn sweep_surplus(env: Env, caller: Address) -> Result<(), CarryError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        if is_realisation_pending(&env) {
            return Err(CarryError::RealisationPending);
        }
        let pool = get_pool(&env)?;
        let borrow_asset = get_borrow_asset(&env)?;
        let yield_pool = get_yield_pool(&env)?;
        let adapter = env.current_contract_address();
        let params = get_params(&env)?;

        let s = read_position(&env, &adapter, &pool, &borrow_asset)?;
        let (d, idle_b) = (s.d, s.idle_b);
        let payout = math::sweep_amount(s.y(), d, params.surplus_buffer_bps)?;
        if payout <= 0 {
            return Err(CarryError::NothingToSweep);
        }
        let yield_cap = withdrawable(&s.y_reserve, s.y_pool)?;
        // Idle b funds the payout first; only the remainder is drawn from
        // the yield leg (within its liquidity cap). Anything still unfunded
        // fails closed with no partial payout.
        let needed = payout - payout.min(idle_b);
        let from_pool = needed.min(yield_cap);
        if needed > from_pool {
            return Err(CarryError::InvalidParams);
        }
        if from_pool > 0 {
            let mut requests = Vec::new(&env);
            requests.push_back(Request {
                request_type: REQUEST_WITHDRAW,
                address: borrow_asset.clone(),
                amount: from_pool,
            });
            env.submit_auth(&yield_pool, &adapter, &requests, None);
        }
        env.storage()
            .instance()
            .set(&DataKey::RealisationPending, &true);
        let recipient = get_surplus_recipient(&env)?;
        let token = soroban_sdk::token::Client::new(&env, &borrow_asset);
        token.transfer(&adapter, &recipient, &payout);
        env.events()
            .publish((symbol_short!("sweep"), recipient), payout);
        Ok(())
    }

    /// Clear `realisation_pending` once the converted c has been donated to
    /// the adapter (admin-only).
    #[allow(deprecated)]
    pub fn clear_realisation_pending(env: Env, caller: Address) -> Result<(), CarryError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        env.storage()
            .instance()
            .set(&DataKey::RealisationPending, &false);
        env.events().publish((symbol_short!("clear"), caller), ());
        Ok(())
    }

    /// Update tunable parameters (admin). Validates against the same hard
    /// bounds as the constructor.
    #[allow(deprecated)]
    pub fn set_params(env: Env, caller: Address, params: Params) -> Result<(), CarryError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        params.validate()?;
        env.storage().instance().set(&DataKey::Params, &params);
        env.events().publish((symbol_short!("params"), caller), ());
        Ok(())
    }

    /// Extend instance storage TTL (admin-only), mirroring the blend-adapter
    /// idiom.
    #[allow(deprecated)]
    pub fn extend_ttl(env: Env, caller: Address) -> Result<(), CarryError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        Ok(())
    }

    #[allow(deprecated)]
    pub fn set_admin(env: Env, caller: Address, admin: Address) -> Result<(), CarryError> {
        extend_instance_ttl(&env);
        propose_admin(&env, &caller, &admin).map_err(Into::into)
    }

    #[allow(deprecated)]
    pub fn accept_admin(env: Env, caller: Address) -> Result<(), CarryError> {
        extend_instance_ttl(&env);
        common_accept_admin(&env, &caller).map_err(Into::into)
    }

    /// The current admin.
    pub fn admin(env: Env) -> Result<Address, CarryError> {
        extend_instance_ttl(&env);
        common_admin(&env).map_err(Into::into)
    }
}

#[contractimpl]
impl Upgradeable for CarryAdapterContract {
    #[allow(deprecated)]
    fn upgrade(e: &Env, new_wasm_hash: BytesN<32>, operator: Address) {
        extend_instance_ttl(e);
        upgrade_as_admin(e, &new_wasm_hash, &operator)
            .unwrap_or_else(|err| panic_with_error!(e, CarryError::from(err)));
    }
}

impl Params {
    fn validate(&self) -> Result<(), CarryError> {
        Ok(math::validate_params(&math::ParamsIn {
            borrow_utilization_bps: self.borrow_utilization_bps,
            surplus_buffer_bps: self.surplus_buffer_bps,
            surplus_haircut_bps: self.surplus_haircut_bps,
            oracle_free_band_bps: self.oracle_free_band_bps,
            oracle_max_age_secs: self.oracle_max_age_secs,
            min_residual_collateral: self.min_residual_collateral,
        })?)
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

fn get_address(env: &Env, key: DataKey) -> Result<Address, CarryError> {
    env.storage()
        .instance()
        .get(&key)
        .ok_or(CarryError::MissingConfig)
}

fn get_oracle(env: &Env, pool: &Address) -> Result<Address, CarryError> {
    Ok(env.pool_config(pool).oracle)
}

fn get_pool(env: &Env) -> Result<Address, CarryError> {
    get_address(env, DataKey::Pool)
}

fn get_yield_pool(env: &Env) -> Result<Address, CarryError> {
    get_address(env, DataKey::YieldPool)
}

fn get_collateral_asset(env: &Env) -> Result<Address, CarryError> {
    get_address(env, DataKey::CollateralAsset)
}

fn get_borrow_asset(env: &Env) -> Result<Address, CarryError> {
    get_address(env, DataKey::BorrowAsset)
}

fn get_surplus_recipient(env: &Env) -> Result<Address, CarryError> {
    get_address(env, DataKey::SurplusRecipient)
}

fn get_params(env: &Env) -> Result<Params, CarryError> {
    env.storage()
        .instance()
        .get(&DataKey::Params)
        .ok_or(CarryError::MissingConfig)
}

fn is_realisation_pending(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&DataKey::RealisationPending)
        .unwrap_or(false)
}

/// Position snapshot in underlying units, with the reserves it was read from
/// so callers never re-fetch them.
pub(crate) struct Snapshot {
    pub c_pool: i128,
    pub idle_c: i128,
    pub d: i128,
    pub idle_b: i128,
    pub y_pool: i128,
    pub c_reserve: Reserve,
    pub b_reserve: Reserve,
    pub y_reserve: Reserve,
}

impl Snapshot {
    /// Total collateral `C = c_pool + idle_c`.
    pub fn c(&self) -> i128 {
        self.c_pool + self.idle_c
    }

    /// Total yield leg `Y = y_pool + idle_b`.
    pub fn y(&self) -> i128 {
        self.y_pool + self.idle_b
    }
}

/// Read the position snapshot; one `get_positions` per pool.
pub(crate) fn read_position(
    env: &Env,
    adapter: &Address,
    pool: &Address,
    borrow_asset: &Address,
) -> Result<Snapshot, CarryError> {
    let collateral_asset = get_collateral_asset(env)?;
    let yield_pool = get_yield_pool(env)?;
    let c_reserve = env.reserve(pool, &collateral_asset);
    let b_reserve = env.reserve(pool, borrow_asset);
    let y_reserve = env.reserve(&yield_pool, borrow_asset);
    let pool_pos = env.positions(pool, adapter);
    let yield_pos = env.positions(&yield_pool, adapter);
    let c_pool = math::b_token_underlying(
        pool_pos.collateral.get(c_reserve.config.index).unwrap_or(0),
        c_reserve.data.b_rate,
    )?;
    let d = math::debt_underlying(
        pool_pos
            .liabilities
            .get(b_reserve.config.index)
            .unwrap_or(0),
        b_reserve.data.d_rate,
    )?;
    let y_pool = math::b_token_underlying(
        yield_pos.supply.get(y_reserve.config.index).unwrap_or(0),
        y_reserve.data.b_rate,
    )?;
    Ok(Snapshot {
        c_pool,
        idle_c: env.balance(&collateral_asset, adapter),
        d,
        idle_b: env.balance(borrow_asset, adapter),
        y_pool,
        c_reserve,
        b_reserve,
        y_reserve,
    })
}

/// Underlying `held` can pull out of `reserve`: held, capped by liquidity.
fn withdrawable(reserve: &Reserve, held: i128) -> Result<i128, CarryError> {
    Ok(math::liquidity_cap(
        reserve_supply(reserve),
        reserve_liabilities(reserve),
        reserve.data.b_rate,
    )?
    .min(held))
}

fn read_prices(
    env: &Env,
    borrow_asset: &Address,
    collateral_asset: &Address,
    params: &Params,
) -> Result<(i128, i128), CarryError> {
    let oracle = get_oracle(env, &get_pool(env)?)?;
    let price_b = read_price(env, &oracle, borrow_asset, params.oracle_max_age_secs)?;
    let price_c = read_price(env, &oracle, collateral_asset, params.oracle_max_age_secs)?;
    Ok((price_b, price_c))
}

fn conv_ctx(
    env: &Env,
    collateral_asset: &Address,
    borrow_asset: &Address,
    s: &Snapshot,
    params: &Params,
) -> Result<ConvCtx, CarryError> {
    let (price_b, price_c) = read_prices(env, borrow_asset, collateral_asset, params)?;
    Ok(ConvCtx {
        price_b,
        price_c,
        dec_b: s.b_reserve.config.decimals,
        dec_c: s.c_reserve.config.decimals,
    })
}

/// `b_target` for the current POOL collateral. `Err(OracleStale)` when the
/// oracle has no usable price.
fn target_borrow_b(
    env: &Env,
    s: &Snapshot,
    collateral_asset: &Address,
    borrow_asset: &Address,
    params: &Params,
) -> Result<i128, CarryError> {
    if s.c_pool <= 0 {
        return Ok(0);
    }
    let ctx = conv_ctx(env, collateral_asset, borrow_asset, s, params)?;
    Ok(math::borrow_target_b(
        s.c_pool,
        &ctx,
        s.c_reserve.config.c_factor,
        s.b_reserve.config.l_factor,
        params.borrow_utilization_bps,
    )?)
}

pub(crate) fn read_price(
    env: &Env,
    oracle: &Address,
    asset: &Address,
    max_age_secs: u64,
) -> Result<i128, CarryError> {
    let client = PriceFeedClient::new(env, oracle);
    let decimals = client.decimals();
    if decimals != math::PRICE_DECIMALS {
        return Err(CarryError::InvalidOracleDecimals);
    }
    // A SEP-40 `lastprice` that returns `None` (unrecorded, stale or future
    // relative to the feed's own freshness threshold) means no usable price:
    // fail closed as stale, not unavailable.
    let price: PriceData = client
        .lastprice(&Asset::Stellar(asset.clone()))
        .ok_or(CarryError::OracleStale)?;
    let now = env.ledger().timestamp();
    if price.timestamp > now || now - price.timestamp > max_age_secs || price.price <= 0 {
        return Err(CarryError::OracleStale);
    }
    Ok(price.price)
}
