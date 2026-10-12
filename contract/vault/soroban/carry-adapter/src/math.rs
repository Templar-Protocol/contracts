//! Pure fixed-point math for the carry adapter.
//!
//! Everything here is plain integer arithmetic on `i128` (with `u128`
//! intermediates and checked operations). There is no Soroban SDK dependency,
//! no storage and no oracle access: the contract reads balances, prices and
//! pool state, feeds them in, and executes whatever these helpers decide.
//!
//! Naming: `c` is the collateral asset (the vault asset), `b` is the borrow
//! asset. `C` = collateral underlying (pool collateral plus idle), `D` = debt
//! in `b` units (rounded up), `Y` = yield leg in `b` units (yield-pool supply
//! plus idle `b`).
//!
//! Rounding policy (always against the adapter, in favour of the pools and of
//! the remaining depositors): collateral and supply underlying round down,
//! debt rounds up, surplus value rounds down, shortfall value rounds up.
//!
//! Every function is total: overflow, negative inputs, zero divisors and bad
//! prices are reported as [`MathError`]; nothing panics.

/// Fixed-point scale of Blend `b_rate` / `d_rate` (12 decimals).
pub const RATE_SCALE: i128 = 1_000_000_000_000;
/// Basis-point denominator.
pub const BPS: i128 = 10_000;
/// Blend 7-decimal scalar: utilization, `c_factor`/`l_factor`, `max_util`.
pub const SCALAR_7: i128 = 10_000_000;
/// Decimals every oracle price must carry.
pub const PRICE_DECIMALS: u32 = 7;
/// Hard cap on `borrow_utilization_bps`.
pub const MAX_BORROW_UTILIZATION_BPS: u32 = 9_000;
/// Hard cap on `surplus_haircut_bps`.
pub const MAX_SURPLUS_HAIRCUT_BPS: u32 = 1_000;
/// Hard cap on `oracle_free_band_bps` (and so on `surplus_buffer_bps`).
pub const MAX_ORACLE_FREE_BAND_BPS: u32 = 50;
/// Hard cap on `oracle_max_age_secs` (one day).
pub const MAX_ORACLE_AGE_SECS: u64 = 86_400;

/// How many times [`liquidity_cap`] may decrement the cap while verifying it
/// against the exact post-withdraw utilization.
const LIQUIDITY_CAP_MAX_DECREMENTS: u32 = 3;

/// Errors returned by the math helpers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathError {
    /// An intermediate or final value does not fit in `u128` / `i128`.
    Overflow,
    /// A divisor was zero.
    DivisionByZero,
    /// A quantity that must be non-negative was negative.
    Negative,
    /// An oracle price was zero or negative.
    InvalidPrice,
    /// `borrow_utilization_bps` exceeds [`MAX_BORROW_UTILIZATION_BPS`].
    InvalidBorrowUtilization,
    /// `surplus_haircut_bps` exceeds [`MAX_SURPLUS_HAIRCUT_BPS`].
    InvalidSurplusHaircut,
    /// `oracle_free_band_bps` exceeds [`MAX_ORACLE_FREE_BAND_BPS`].
    InvalidOracleBand,
    /// `surplus_buffer_bps` exceeds `oracle_free_band_bps`.
    InvalidSurplusBuffer,
    /// `oracle_max_age_secs` exceeds [`MAX_ORACLE_AGE_SECS`].
    InvalidOracleAge,
    /// `min_residual_collateral` is negative.
    InvalidMinResidual,
}

/// Rounding direction for a division.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Round {
    /// Round towards zero (all quantities here are non-negative).
    Floor,
    /// Round away from zero.
    Ceil,
}

// ---------------------------------------------------------------------------
// Checked primitives
// ---------------------------------------------------------------------------

fn nn(v: i128) -> Result<u128, MathError> {
    u128::try_from(v).map_err(|_| MathError::Negative)
}

fn add(a: i128, b: i128) -> Result<i128, MathError> {
    a.checked_add(b).ok_or(MathError::Overflow)
}

fn sub(a: i128, b: i128) -> Result<i128, MathError> {
    a.checked_sub(b).ok_or(MathError::Overflow)
}

fn mul(a: i128, b: i128) -> Result<i128, MathError> {
    a.checked_mul(b).ok_or(MathError::Overflow)
}

fn pow10(exp: u32) -> Result<i128, MathError> {
    10i128.checked_pow(exp).ok_or(MathError::Overflow)
}

/// `a * b * c / den` on non-negative values with a `u128` intermediate.
fn mul_div3(a: i128, b: i128, c: i128, den: i128, round: Round) -> Result<i128, MathError> {
    let c_u = nn(c)?;
    let prod = nn(a)?
        .checked_mul(nn(b)?)
        .and_then(|p| p.checked_mul(c_u))
        .ok_or(MathError::Overflow)?;
    let den = nn(den)?;
    if den == 0 {
        return Err(MathError::DivisionByZero);
    }
    let q = prod / den;
    let q = if round == Round::Ceil && prod % den != 0 {
        q.checked_add(1).ok_or(MathError::Overflow)?
    } else {
        q
    };
    i128::try_from(q).map_err(|_| MathError::Overflow)
}

/// `a * b / den` on non-negative values.
fn mul_div(a: i128, b: i128, den: i128, round: Round) -> Result<i128, MathError> {
    mul_div3(a, b, 1, den, round)
}

// ---------------------------------------------------------------------------
// Pool token <-> underlying
// ---------------------------------------------------------------------------

/// Underlying of `b_tokens` at `b_rate` (12 decimals), rounded **down**
/// (the pool's own `to_asset_from_b_token`). Used for both the collateral
/// position and the yield-pool supply.
pub fn b_token_underlying(b_tokens: i128, b_rate: i128) -> Result<i128, MathError> {
    mul_div(b_tokens, b_rate, RATE_SCALE, Round::Floor)
}

/// Debt underlying of `d_tokens` at `d_rate` (12 decimals), rounded **up**
/// (the pool's own `to_asset_from_d_token`).
pub fn debt_underlying(d_tokens: i128, d_rate: i128) -> Result<i128, MathError> {
    mul_div(d_tokens, d_rate, RATE_SCALE, Round::Ceil)
}

// ---------------------------------------------------------------------------
// Price conversion
// ---------------------------------------------------------------------------

/// Prices (7 decimals, quote-asset terms) and token decimals needed to convert
/// between the borrow asset `b` and the collateral asset `c`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConvCtx {
    /// Oracle price of one whole `b` token, 7 decimals.
    pub price_b: i128,
    /// Oracle price of one whole `c` token, 7 decimals.
    pub price_c: i128,
    /// Decimals of the `b` token.
    pub dec_b: u32,
    /// Decimals of the `c` token.
    pub dec_c: u32,
}

impl ConvCtx {
    /// Value of `amount_b` (in `b` base units) in `c` base units:
    /// `amount_b * price_b * 10^dec_c / (price_c * 10^dec_b)`.
    ///
    /// Use [`Round::Floor`] for surplus (value the adapter receives) and
    /// [`Round::Ceil`] for shortfall (value the adapter owes).
    pub fn b_to_c(&self, amount_b: i128, round: Round) -> Result<i128, MathError> {
        conv(
            amount_b,
            self.price_b,
            self.price_c,
            self.dec_c,
            self.dec_b,
            round,
        )
    }

    /// Value of `amount_c` (in `c` base units) in `b` base units:
    /// `amount_c * price_c * 10^dec_b / (price_b * 10^dec_c)`.
    pub fn c_to_b(&self, amount_c: i128, round: Round) -> Result<i128, MathError> {
        conv(
            amount_c,
            self.price_c,
            self.price_b,
            self.dec_b,
            self.dec_c,
            round,
        )
    }
}

/// `amount * num_price / den_price`, adjusted from `den_dec` to `num_dec`
/// decimals, with the decimal power folded to whichever side keeps the
/// intermediate smaller.
fn conv(
    amount: i128,
    num_price: i128,
    den_price: i128,
    num_dec: u32,
    den_dec: u32,
    round: Round,
) -> Result<i128, MathError> {
    if num_price <= 0 || den_price <= 0 {
        return Err(MathError::InvalidPrice);
    }
    let (num_exp, den_exp) = if num_dec >= den_dec {
        (pow10(num_dec - den_dec)?, 1)
    } else {
        (1, pow10(den_dec - num_dec)?)
    };
    let den = mul(den_price, den_exp)?;
    mul_div3(amount, num_price, num_exp, den, round)
}

// (conv wrappers live on `ConvCtx`)

// ---------------------------------------------------------------------------
// Borrow sizing
// ---------------------------------------------------------------------------

/// Target debt, in `b` units, for the pool collateral position after a supply:
/// `floor(C_pool_after_value * c_factor * l_factor * util_bps / 1e4)`.
///
/// `c_pool_after_c` is the pool collateral underlying in `c` units *after* the
/// `SupplyCollateral` of this call. `c_factor` / `l_factor` are the Blend
/// reserve factors in 7 decimals (`1e7` = 100%). The value is computed in two
/// floors (conversion, then factors), so it never exceeds the exact value.
pub fn borrow_target_b(
    c_pool_after_c: i128,
    ctx: &ConvCtx,
    c_factor: u32,
    l_factor: u32,
    util_bps: u32,
) -> Result<i128, MathError> {
    let value_b = ctx.c_to_b(c_pool_after_c, Round::Floor)?;
    let factors = mul(
        mul(i128::from(c_factor), i128::from(l_factor))?,
        i128::from(util_bps),
    )?;
    let den = mul(mul(SCALAR_7, SCALAR_7)?, BPS)?;
    mul_div(value_b, factors, den, Round::Floor)
}

/// Largest extra borrow the borrow reserve accepts before its own utilization
/// exceeds `max_util` (7 decimals): `max(0, floor(supply * max_util / 1e7) - liab)`.
///
/// This is exact for the pool's `ceil(liab' * 1e7 / supply) <= max_util` check
/// on the underlying amount; the d-token mint rounds the booked debt up, so
/// the caller should treat the result as the clamp, not as slack.
pub fn max_util_headroom(
    reserve_supply: i128,
    reserve_liab: i128,
    max_util: u32,
) -> Result<i128, MathError> {
    nn(reserve_liab)?;
    let allowed = mul_div(reserve_supply, i128::from(max_util), SCALAR_7, Round::Floor)?;
    Ok(sub(allowed, reserve_liab)?.max(0))
}

/// Amount to borrow now: `clamp(b_target - D, 0, headroom)`.
pub fn borrow_amount(b_target: i128, d: i128, headroom: i128) -> Result<i128, MathError> {
    nn(b_target)?;
    nn(d)?;
    Ok(sub(b_target, d)?.clamp(0, headroom.max(0)))
}

// ---------------------------------------------------------------------------
// Liquidity cap
// ---------------------------------------------------------------------------

/// Whether a pool `Withdraw` leaving `supply` / `liab` passes the pool's
/// post-state check (`utilization < 100%`, utilization = `ceil(liab*1e7/supply)`,
/// saturating at 100% when `liab >= supply`, 0 when `liab == 0`).
fn post_withdraw_ok(supply: i128, liab: i128) -> Result<bool, MathError> {
    if liab == 0 {
        return Ok(true);
    }
    if supply <= 0 || liab >= supply {
        return Ok(false);
    }
    Ok(mul_div(liab, SCALAR_7, supply, Round::Ceil)? < SCALAR_7)
}

/// Largest amount (underlying) that can be withdrawn from a reserve with total
/// `supply` underlying, `liab` underlying liabilities and `b_rate` (12 decimals)
/// without the pool panicking on its post-state `utilization >= 100%` check.
///
/// `cap = max(0, supply - liab - ceil(liab / (1e7 - 1)) - ceil(b_rate / 1e12) - 1)`,
/// then checked against the exact post-state utilization and decremented at
/// most three times, otherwise `0`. The `ceil(liab/(1e7-1))` term covers the
/// pool's utilization rounding (`ceil(liab*1e7/supply') < 1e7` needs
/// `supply' >= liab * 1e7/(1e7-1)`); the `b_rate` term covers burning
/// `ceil(amount/b_rate)` b-tokens; the last `1` covers the floor in
/// `total_supply`. With `liab == 0` utilization is 0 whatever is withdrawn, so
/// the whole `supply` is available.
///
/// The caller still clamps to its own holdings.
pub fn liquidity_cap(supply: i128, liab: i128, b_rate: i128) -> Result<i128, MathError> {
    nn(liab)?;
    nn(b_rate)?;
    if supply <= 0 {
        return Ok(0);
    }
    if liab == 0 {
        return Ok(supply);
    }
    let liab_term = mul_div(liab, 1, SCALAR_7 - 1, Round::Ceil)?;
    let rate_term = mul_div(b_rate, 1, RATE_SCALE, Round::Ceil)?;
    let reserved = add(add(add(liab, liab_term)?, rate_term)?, 1)?;
    let mut cap = sub(supply, reserved)?.max(0);
    for _ in 0..=LIQUIDITY_CAP_MAX_DECREMENTS {
        if cap == 0 {
            return Ok(0);
        }
        if post_withdraw_ok(sub(supply, cap)?, liab)? {
            return Ok(cap);
        }
        cap = sub(cap, 1)?;
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// Unwind
// ---------------------------------------------------------------------------

/// Inputs to [`unwind_split`]. All quantities are non-negative; collateral
/// quantities are `c` units, debt/yield quantities are `b` units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnwindIn {
    /// Collateral the vault asked for.
    pub amount: i128,
    /// Total collateral `C` = `c_pool + idle_c`.
    pub c: i128,
    /// Collateral supplied to `pool` (underlying).
    pub c_pool: i128,
    /// Idle collateral held by the adapter.
    pub idle_c: i128,
    /// Debt `D`.
    pub d: i128,
    /// Idle borrow asset held by the adapter.
    pub idle_b: i128,
    /// Borrow asset withdrawable from the yield pool right now (already clamped
    /// to the yield-pool liquidity cap and the adapter's own holdings).
    pub yield_withdrawable: i128,
    /// Collateral the pool can release right now ([`liquidity_cap`] of the
    /// collateral reserve, clamped to the adapter's holdings).
    pub pool_cap: i128,
    /// Residual collateral the pool needs to keep the position valid
    /// (heuristic; the pool's `min_collateral` check has the last word).
    pub min_residual: i128,
}

/// Result of [`unwind_split`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnwindOut {
    /// Collateral delivered from the adapter's idle balance.
    pub x_idle: i128,
    /// Collateral withdrawn from `pool` (`x = x_idle + x_pool`).
    pub x_pool: i128,
    /// Debt to repay (borrow-asset amount).
    pub r: i128,
}

/// Plans a pro-rata unwind.
///
/// `x_req = min(amount, C, idle_c + min(c_pool, pool_cap))`,
/// `r = ceil(D * x_req / C)` (0 when `D == 0`). Repayment is funded from idle
/// `b`, then from the yield pool. If funding `f < r`, the release shrinks to
/// `x = floor(C * f / D)` and `r = f` (so health never worsens). If debt would
/// remain and the leftover collateral `C - x` would drop below
/// `min_residual`, `x` is reduced to `max(0, C - min_residual)`. Collateral is
/// delivered idle-first: `x_idle = min(x, idle_c)`, `x_pool = x - x_idle`.
///
/// `D > 0` with nothing fundable returns all zeros; the contract maps that to
/// `ZeroWithdrawal`.
pub fn unwind_split(i: &UnwindIn) -> Result<UnwindOut, MathError> {
    for v in [
        i.amount,
        i.c,
        i.c_pool,
        i.idle_c,
        i.d,
        i.idle_b,
        i.yield_withdrawable,
        i.pool_cap,
        i.min_residual,
    ] {
        nn(v)?;
    }
    let zero = UnwindOut {
        x_idle: 0,
        x_pool: 0,
        r: 0,
    };

    let deliverable = add(i.idle_c, i.c_pool.min(i.pool_cap))?;
    let x_req = i.amount.min(i.c).min(deliverable);
    if x_req == 0 {
        return Ok(zero);
    }

    let mut r = if i.d == 0 {
        0
    } else {
        mul_div(i.d, x_req, i.c, Round::Ceil)?
    };
    let funded = r.min(add(i.idle_b, i.yield_withdrawable)?);
    let mut x = x_req;
    if funded < r {
        // Not enough funding for the pro-rata repayment: release only what the
        // funded repayment justifies (rounded down in the pool's favour).
        x = mul_div(i.c, funded, i.d, Round::Floor)?;
        r = funded;
    }
    if sub(i.d, r)? > 0 && sub(i.c, x)? < i.min_residual {
        x = x.min(sub(i.c, i.min_residual)?.max(0));
    }

    let x_idle = x.min(i.idle_c);
    let x_pool = sub(x, x_idle)?;
    Ok(UnwindOut { x_idle, x_pool, r })
}

// ---------------------------------------------------------------------------
// NAV
// ---------------------------------------------------------------------------

/// Whether `|Y - D|` is inside the oracle-free band: `|Y - D| <= ceil(band_bps * D / 1e4)`.
///
/// The threshold is rounded **up** so that the post-[`sweep_amount`] state
/// (`Y - D = ceil(D * buffer / 1e4)` with `buffer <= band`) is always inside
/// the band; the excluded value is therefore at most `band_bps * D / 1e4`
/// rounded up to a whole unit. With `D == 0` only `Y == 0` is in band.
pub fn nav_in_band(y: i128, d: i128, band_bps: u32) -> Result<bool, MathError> {
    nn(y)?;
    nn(d)?;
    let diff = y.abs_diff(d);
    let threshold = nn(mul_div(d, i128::from(band_bps), BPS, Round::Ceil)?)?;
    Ok(diff <= threshold)
}

/// NAV when priced: `C + conv(Y - D)`, floored at 0.
///
/// Surplus (`Y >= D`): `floor(floor(conv(Y - D)) * (1e4 - haircut_bps) / 1e4)` is
/// added. Shortfall (`Y < D`): `ceil(conv(D - Y))` is deducted. The result
/// never exceeds the exact (rational) value of the same expression.
pub fn nav_priced(
    c: i128,
    y: i128,
    d: i128,
    haircut_bps: u32,
    ctx: &ConvCtx,
) -> Result<i128, MathError> {
    nn(c)?;
    nn(y)?;
    nn(d)?;
    if haircut_bps > MAX_SURPLUS_HAIRCUT_BPS {
        return Err(MathError::InvalidSurplusHaircut);
    }
    if y >= d {
        let surplus = ctx.b_to_c(sub(y, d)?, Round::Floor)?;
        let kept = mul_div(
            surplus,
            sub(BPS, i128::from(haircut_bps))?,
            BPS,
            Round::Floor,
        )?;
        add(c, kept)
    } else {
        let shortfall = ctx.b_to_c(sub(d, y)?, Round::Ceil)?;
        Ok(sub(c, shortfall)?.max(0))
    }
}

// ---------------------------------------------------------------------------
// Surplus sweep and parameters
// ---------------------------------------------------------------------------

/// Borrow-asset amount `sweep_surplus` may pay out:
/// `max(0, Y - ceil(D * (1e4 + buffer_bps) / 1e4))`.
pub fn sweep_amount(y: i128, d: i128, buffer_bps: u32) -> Result<i128, MathError> {
    nn(y)?;
    nn(d)?;
    let keep = mul_div(d, add(BPS, i128::from(buffer_bps))?, BPS, Round::Ceil)?;
    Ok(sub(y, keep)?.max(0))
}

/// Admin-tunable parameters checked by [`validate_params`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParamsIn {
    /// Share of Blend borrow capacity to use, in bps.
    pub borrow_utilization_bps: u32,
    /// Part of `D` kept back by `sweep_surplus`, in bps.
    pub surplus_buffer_bps: u32,
    /// Haircut on priced surplus in NAV, in bps.
    pub surplus_haircut_bps: u32,
    /// Oracle-free band around `Y == D`, in bps of `D`.
    pub oracle_free_band_bps: u32,
    /// Maximum oracle price age in seconds.
    pub oracle_max_age_secs: u64,
    /// Collateral the unwind heuristic tries to leave behind while debt remains.
    pub min_residual_collateral: i128,
}

/// Enforces the hard constants and `surplus_buffer_bps <= oracle_free_band_bps`
/// (so the post-sweep steady state stays inside the band).
pub fn validate_params(p: &ParamsIn) -> Result<(), MathError> {
    if p.borrow_utilization_bps > MAX_BORROW_UTILIZATION_BPS {
        return Err(MathError::InvalidBorrowUtilization);
    }
    if p.surplus_haircut_bps > MAX_SURPLUS_HAIRCUT_BPS {
        return Err(MathError::InvalidSurplusHaircut);
    }
    if p.oracle_free_band_bps > MAX_ORACLE_FREE_BAND_BPS {
        return Err(MathError::InvalidOracleBand);
    }
    if p.surplus_buffer_bps > p.oracle_free_band_bps {
        return Err(MathError::InvalidSurplusBuffer);
    }
    if p.oracle_max_age_secs > MAX_ORACLE_AGE_SECS {
        return Err(MathError::InvalidOracleAge);
    }
    if p.min_residual_collateral < 0 {
        return Err(MathError::InvalidMinResidual);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use proptest::prelude::*;

    const E12: i128 = 1_000_000_000_000;
    const E18: i128 = 1_000_000_000_000_000_000;

    // ----- test oracle: the pool's own reserve arithmetic ------------------

    /// Mirrors `Reserve::{total_supply, total_liabilities, utilization}` and
    /// the b-token burn of `Withdraw` (`to_b_token_up`).
    #[derive(Debug, Clone, Copy)]
    struct PoolModel {
        b_supply: i128,
        b_rate: i128,
        d_supply: i128,
        d_rate: i128,
    }

    impl PoolModel {
        fn supply(&self) -> i128 {
            self.b_supply * self.b_rate / E12
        }
        fn liab(&self) -> i128 {
            (self.d_supply * self.d_rate + E12 - 1) / E12
        }
        fn utilization(&self) -> i128 {
            let (l, s) = (self.liab(), self.supply());
            if l == 0 {
                0
            } else if l >= s {
                SCALAR_7
            } else {
                (l * SCALAR_7 + s - 1) / s
            }
        }
        /// `true` if `Withdraw(amount)` passes the post-state `< 100%` check.
        fn withdraw_ok(&self, amount: i128) -> bool {
            let burn = (amount * E12 + self.b_rate - 1) / self.b_rate;
            if burn > self.b_supply {
                return false;
            }
            let after = PoolModel {
                b_supply: self.b_supply - burn,
                ..*self
            };
            after.utilization() < SCALAR_7
        }
    }

    fn ctx(pb: i128, pc: i128, db: u32, dc: u32) -> ConvCtx {
        ConvCtx {
            price_b: pb,
            price_c: pc,
            dec_b: db,
            dec_c: dc,
        }
    }

    fn unit_ctx() -> ConvCtx {
        ctx(SCALAR_7, SCALAR_7, 7, 7)
    }

    // ----- rates and conversion --------------------------------------------

    #[test]
    fn rate_rounding_directions() {
        assert_eq!(b_token_underlying(3, E12 + E12 / 2), Ok(4)); // 4.5 -> 4
        assert_eq!(debt_underlying(3, E12 + E12 / 2), Ok(5)); // 4.5 -> 5
        assert_eq!(debt_underlying(0, 2 * E12), Ok(0));
        assert_eq!(b_token_underlying(-1, E12), Err(MathError::Negative));
        assert_eq!(
            b_token_underlying(i128::MAX, i128::MAX),
            Err(MathError::Overflow)
        );
    }

    #[test]
    fn conversion_known_values_and_rounding() {
        // 1000 b (7 dec) at $1 into c (8 dec) at $100_000 = 0.01 c = 1_000_000 units.
        let c = ctx(SCALAR_7, 100_000 * SCALAR_7, 7, 8);
        assert_eq!(c.b_to_c(10_000_000_000, Round::Floor), Ok(1_000_000));
        assert_eq!(c.c_to_b(1_000_000, Round::Floor), Ok(10_000_000_000));
        // sub-unit values round by direction
        assert_eq!(c.b_to_c(1_000, Round::Floor), Ok(0));
        assert_eq!(c.b_to_c(1_000, Round::Ceil), Ok(1));
        assert_eq!(c.b_to_c(0, Round::Ceil), Ok(0));
        // dec_c < dec_b path
        let c2 = ctx(SCALAR_7, SCALAR_7, 8, 6); // b 8 dec, c 6 dec, same price
        assert_eq!(c2.b_to_c(1_050, Round::Floor), Ok(10));
        assert_eq!(c2.b_to_c(1_050, Round::Ceil), Ok(11));
        assert_eq!(c2.c_to_b(10, Round::Floor), Ok(1_000));
    }

    #[test]
    fn conversion_rejects_bad_inputs() {
        let mk = |pb, pc, db, dc| ConvCtx {
            price_b: pb,
            price_c: pc,
            dec_b: db,
            dec_c: dc,
        };
        assert_eq!(
            mk(0, 1, 7, 7).b_to_c(1, Round::Floor),
            Err(MathError::InvalidPrice)
        );
        assert_eq!(
            mk(1, -1, 7, 7).b_to_c(1, Round::Floor),
            Err(MathError::InvalidPrice)
        );
        assert_eq!(
            mk(1, 1, 7, 7).b_to_c(-1, Round::Floor),
            Err(MathError::Negative)
        );
        assert_eq!(
            mk(1, 1, 0, 40).b_to_c(1, Round::Floor),
            Err(MathError::Overflow)
        );
        assert_eq!(
            mk(i128::MAX, 1, 7, 7).b_to_c(i128::MAX, Round::Floor),
            Err(MathError::Overflow)
        );
    }

    // ----- borrow sizing -----------------------------------------------------

    #[test]
    fn borrow_target_and_amount() {
        // 100 c worth 100 b; c_factor 75%, l_factor 90%, util 80%:
        // 100 * 0.75 * 0.9 * 0.8 = 54
        let t = borrow_target_b(100_0000000, &unit_ctx(), 7_500_000, 9_000_000, 8_000).unwrap();
        assert_eq!(t, 54_0000000);
        // flooring: tiny collateral never rounds up
        assert_eq!(
            borrow_target_b(1, &unit_ctx(), 7_500_000, 9_000_000, 8_000),
            Ok(0)
        );
        assert_eq!(borrow_amount(54, 10, 100), Ok(44));
        assert_eq!(borrow_amount(54, 10, 20), Ok(20));
        assert_eq!(borrow_amount(54, 60, 100), Ok(0));
        assert_eq!(borrow_amount(54, 10, -5), Ok(0));
    }

    #[test]
    fn max_util_headroom_cases() {
        assert_eq!(max_util_headroom(1_000, 100, 9_000_000), Ok(800));
        assert_eq!(max_util_headroom(1_000, 900, 9_000_000), Ok(0));
        assert_eq!(max_util_headroom(1_000, 950, 9_000_000), Ok(0));
        assert_eq!(max_util_headroom(0, 0, 9_000_000), Ok(0));
        assert_eq!(
            max_util_headroom(1_000, -1, 9_000_000),
            Err(MathError::Negative)
        );
    }

    // ----- liquidity cap -----------------------------------------------------

    #[test]
    fn liquidity_cap_boundary_liab_1e7() {
        // liab = 1e7, b_rate 1.0: the naive cap `supply - liab - 1` leaves
        // supply' = liab + 1 and ceil(1e14 / (1e7 + 1)) = 1e7 -> pool panics.
        let supply = 20_000_000;
        let liab = 10_000_000;
        let naive_after = liab + 1;
        let model = PoolModel {
            b_supply: naive_after,
            b_rate: E12,
            d_supply: liab,
            d_rate: E12,
        };
        assert_eq!(
            model.utilization(),
            SCALAR_7,
            "naive cap hits 100% utilization"
        );

        let cap = liquidity_cap(supply, liab, E12).unwrap();
        assert!(cap < supply - liab - 1);
        let full = PoolModel {
            b_supply: supply,
            b_rate: E12,
            d_supply: liab,
            d_rate: E12,
        };
        assert!(full.withdraw_ok(cap));
        assert!(cap > 0);
    }

    #[test]
    fn liquidity_cap_high_b_rate() {
        for rate in [2 * E12, 2 * E12 + 1, 3 * E12 + E12 / 3, 7 * E12] {
            for liab in [1, 9_999_999, 10_000_000, 10_000_001, 123_456_789_012] {
                let b_supply = 1_000_000_000_000;
                let d_rate = E12;
                let m = PoolModel {
                    b_supply,
                    b_rate: rate,
                    d_supply: liab,
                    d_rate,
                };
                let cap = liquidity_cap(m.supply(), m.liab(), rate).unwrap();
                if cap > 0 {
                    assert!(m.withdraw_ok(cap), "rate {rate} liab {liab} cap {cap}");
                }
            }
        }
    }

    #[test]
    fn liquidity_cap_edges() {
        assert_eq!(liquidity_cap(0, 0, E12), Ok(0));
        assert_eq!(liquidity_cap(-5, 0, E12), Ok(0));
        // no liabilities: everything is withdrawable
        assert_eq!(liquidity_cap(1_000, 0, 5 * E12), Ok(1_000));
        // fully utilized or insolvent reserve: nothing is withdrawable
        assert_eq!(liquidity_cap(1_000, 1_000, E12), Ok(0));
        assert_eq!(liquidity_cap(1_000, 2_000, E12), Ok(0));
        // cap formula yields 2; supply'=1003 then leaves
        // ceil(1000e7/1003)=9970090 < 1e7, so the 2 stands and the pool
        // accepts the withdrawal (it is not the panic boundary).
        assert_eq!(liquidity_cap(1_005, 1_000, E12), Ok(2));
        assert_eq!(liquidity_cap(1, 1, E12), Ok(0));
        assert_eq!(
            liquidity_cap(i128::MAX, i128::MAX / 2, E12),
            Err(MathError::Overflow)
        );
        assert_eq!(liquidity_cap(1, -1, E12), Err(MathError::Negative));
    }

    // ----- unwind --------------------------------------------------------------

    fn base_in() -> UnwindIn {
        UnwindIn {
            amount: 100,
            c: 1_000,
            c_pool: 800,
            idle_c: 200,
            d: 500,
            idle_b: 10,
            yield_withdrawable: 1_000,
            pool_cap: 800,
            min_residual: 0,
        }
    }

    #[test]
    fn unwind_pro_rata_and_idle_first() {
        let o = unwind_split(&base_in()).unwrap();
        assert_eq!(o.r, 50); // ceil(500*100/1000)
        assert_eq!((o.x_idle, o.x_pool), (100, 0)); // idle covers it all

        let o = unwind_split(&UnwindIn {
            amount: 450,
            ..base_in()
        })
        .unwrap();
        assert_eq!((o.x_idle, o.x_pool), (200, 250));
        assert_eq!(o.r, 225);
    }

    #[test]
    fn unwind_no_debt_and_full_exit() {
        let o = unwind_split(&UnwindIn {
            d: 0,
            amount: 5_000,
            ..base_in()
        })
        .unwrap();
        assert_eq!((o.r, o.x_idle, o.x_pool), (0, 200, 800)); // x == C

        let o = unwind_split(&UnwindIn {
            amount: 5_000,
            ..base_in()
        })
        .unwrap();
        assert_eq!((o.r, o.x_idle + o.x_pool), (500, 1_000)); // x == C and r == D
    }

    #[test]
    fn unwind_partial_funding_and_zero() {
        // r = 500 but only 10 + 40 fundable -> x = floor(1000*50/500) = 100
        let i = UnwindIn {
            amount: 1_000,
            yield_withdrawable: 40,
            ..base_in()
        };
        let o = unwind_split(&i).unwrap();
        assert_eq!((o.r, o.x_idle + o.x_pool), (50, 100));

        // nothing fundable while debt exists -> all zeros (ZeroWithdrawal upstream)
        let i = UnwindIn {
            idle_b: 0,
            yield_withdrawable: 0,
            ..base_in()
        };
        let o = unwind_split(&i).unwrap();
        assert_eq!(
            o,
            UnwindOut {
                x_idle: 0,
                x_pool: 0,
                r: 0
            }
        );
    }

    #[test]
    fn unwind_respects_liquidity_cap_and_residual() {
        // pool can only release 30: x_req = idle 200 + 30
        let i = UnwindIn {
            amount: 1_000,
            pool_cap: 30,
            ..base_in()
        };
        let o = unwind_split(&i).unwrap();
        assert_eq!((o.x_idle, o.x_pool), (200, 30));
        assert_eq!(o.r, 115); // ceil(500*230/1000)

        // residual heuristic: C - x = 900 < 950 while debt remains -> x = 50
        let i = UnwindIn {
            min_residual: 950,
            ..base_in()
        };
        let o = unwind_split(&i).unwrap();
        assert_eq!(o.x_idle + o.x_pool, 50);
        // residual larger than C releases nothing
        let i = UnwindIn {
            min_residual: 5_000,
            ..base_in()
        };
        let o = unwind_split(&i).unwrap();
        assert_eq!(o.x_idle + o.x_pool, 0);
        // no remaining debt: residual does not apply
        let i = UnwindIn {
            d: 0,
            min_residual: 5_000,
            ..base_in()
        };
        assert_eq!(unwind_split(&i).unwrap().x_idle, 100);
    }

    #[test]
    fn unwind_rejects_negative_and_overflow() {
        assert_eq!(
            unwind_split(&UnwindIn { d: -1, ..base_in() }),
            Err(MathError::Negative)
        );
        // idle_b + yield_withdrawable exceeds i128, so the funding sum overflows
        let i = UnwindIn {
            c: i128::MAX,
            c_pool: i128::MAX,
            idle_c: 0,
            pool_cap: i128::MAX,
            amount: i128::MAX,
            d: i128::MAX,
            idle_b: i128::MAX / 2,
            yield_withdrawable: i128::MAX / 2,
            min_residual: 0,
        };
        assert_eq!(unwind_split(&i), Err(MathError::Overflow));
    }

    // ----- NAV -----------------------------------------------------------------

    #[test]
    fn nav_band_and_priced_cases() {
        let u = unit_ctx();
        // in band: d=1e6, band 50 -> threshold 5000
        assert_eq!(nav_in_band(1_004_000, 1_000_000, 50), Ok(true));
        // edge: exactly threshold is in band, one more is out
        assert_eq!(nav_in_band(1_005_000, 1_000_000, 50), Ok(true));
        assert_eq!(nav_in_band(1_005_001, 1_000_000, 50), Ok(false));
        assert_eq!(nav_in_band(994_999, 1_000_000, 50), Ok(false));
        // out of band: surplus 20_000 with 10% haircut -> 18_000
        assert_eq!(nav_in_band(1_020_000, 1_000_000, 50), Ok(false));
        assert_eq!(
            nav_priced(1_000_000, 1_020_000, 1_000_000, 1_000, &u),
            Ok(1_018_000)
        );
        // shortfall deducted in full, haircut irrelevant
        assert_eq!(
            nav_priced(1_000_000, 980_000, 1_000_000, 1_000, &u),
            Ok(980_000)
        );
        // shortfall larger than collateral floors at 0
        assert_eq!(nav_priced(10, 0, 1_000, 0, &u), Ok(0));
        // d == 0: only y == 0 is in band
        assert_eq!(nav_in_band(0, 0, 0), Ok(true));
        assert_eq!(nav_in_band(1, 0, 50), Ok(false));
        assert_eq!(nav_priced(5, 1, 0, 0, &u), Ok(6));
        // band 0 means exact equality
        assert_eq!(nav_in_band(100, 100, 0), Ok(true));
        assert_eq!(nav_in_band(101, 100, 0), Ok(false));
    }

    #[test]
    fn nav_priced_rounding_directions() {
        // b worth 1/10 c unit per base unit: surplus 5 b -> 0.5 c -> 0; shortfall 5 b -> 1
        let c = ctx(SCALAR_7, 10 * SCALAR_7, 7, 7);
        assert_eq!(nav_priced(100, 15, 10, 0, &c), Ok(100));
        assert_eq!(nav_priced(100, 10, 15, 0, &c), Ok(99));
        assert_eq!(
            nav_priced(100, 15, 10, 1_001, &c),
            Err(MathError::InvalidSurplusHaircut)
        );
    }

    // ----- sweep and params ------------------------------------------------------

    #[test]
    fn sweep_amount_cases() {
        assert_eq!(sweep_amount(1_100, 1_000, 0), Ok(100));
        assert_eq!(sweep_amount(1_100, 1_000, 50), Ok(95)); // keep ceil(1005)
        assert_eq!(sweep_amount(1_003, 1_000, 50), Ok(0));
        assert_eq!(sweep_amount(500, 1_000, 50), Ok(0));
        assert_eq!(sweep_amount(7, 0, 50), Ok(7));
        assert_eq!(sweep_amount(1, 1, 50), Ok(0)); // keep ceil(1.005) = 2
    }

    // ----- property tests -------------------------------------------------------

    /// 256-bit exact product `(hi, lo)` of two `u128`s, for cross-multiplication
    /// comparisons in property tests whose intermediates exceed `u128`.
    fn wide_mul(a: u128, b: u128) -> (u128, u128) {
        let mask = u128::from(u64::MAX);
        let (a0, a1) = (a & mask, a >> 64);
        let (b0, b1) = (b & mask, b >> 64);
        let (p00, p01) = (a0 * b0, a0 * b1);
        let (p10, p11) = (a1 * b0, a1 * b1);
        let mid = (p00 >> 64) + (p01 & mask) + (p10 & mask);
        (
            (p11 + (p01 >> 64) + (p10 >> 64) + (mid >> 64)),
            (p00 & mask) | ((mid & mask) << 64),
        )
    }

    /// Amounts with a healthy share of zeros.
    fn amt(max: i128) -> BoxedStrategy<i128> {
        prop_oneof![1 => Just(0i128), 8 => 0..=max].boxed()
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 2_000, failure_persistence: None, ..ProptestConfig::default() })]

        #[test]
        fn rate_conversions_bracket_exact(t in 0..=E18, rate in E12..=9 * E12) {
            let down = b_token_underlying(t, rate).unwrap();
            let up = debt_underlying(t, rate).unwrap();
            prop_assert!(down <= up && up - down <= 1);
            prop_assert!(down * E12 <= t * rate && up * E12 >= t * rate);
        }

        #[test]
        fn conversion_rounding_and_roundtrip(
            a in 0..=E18, pb in 1..=100_000_000_000i128, pc in 1..=100_000_000_000i128,
            db in 0u32..=12, dc in 0u32..=12,
        ) {
            let c = ctx(pb, pc, db, dc);
            // `Overflow` is the function's documented answer for values outside
            // the i128 domain; only the in-domain part of the argument holds.
            let Ok(f) = c.b_to_c(a, Round::Floor) else { return Ok(()) };
            let Ok(u) = c.b_to_c(a, Round::Ceil) else { return Ok(()) };
            prop_assert!(f <= u && u - f <= 1);
            // c -> b -> c never inflates value under floor rounding
            let Ok(back) = c.c_to_b(f, Round::Floor) else { return Ok(()) };
            let Ok(there) = c.b_to_c(back, Round::Floor) else { return Ok(()) };
            prop_assert!(there <= f);
            let Ok(fb) = c.c_to_b(a, Round::Floor) else { return Ok(()) };
            let Ok(ub) = c.c_to_b(a, Round::Ceil) else { return Ok(()) };
            prop_assert!(fb <= ub && ub - fb <= 1);
        }

        #[test]
        fn borrow_amount_within_bounds(t in 0..=E18, d in 0..=E18, h in -5..=E18) {
            let b = borrow_amount(t, d, h).unwrap();
            prop_assert!(b >= 0 && b <= h.max(0) && b <= (t - d).max(0));
            if t - d >= 0 && t - d <= h { prop_assert_eq!(b, t - d); }
        }

        #[test]
        fn borrow_target_never_exceeds_exact(
            c in 0..=1_000_000_000i128, pb in 1..=100_000_000i128, pc in 1..=100_000_000i128,
            db in 0u32..=9, dc in 0u32..=6,
            cf in 0u32..=10_000_000, lf in 0u32..=10_000_000, util in 0u32..=9_000,
        ) {
            let x = ctx(pb, pc, db, dc);
            // `Overflow` is the function's documented answer when the target
            // exceeds the i128 domain; only the in-domain part is asserted.
            let Ok(t) = borrow_target_b(c, &x, cf, lf, util) else { return Ok(()) };
            // exact: c*pc*10^db * cf*lf*util / (pb*10^dc * 1e18), compared by cross-multiplication
            // in 256 bits: t*(pb*10^dc*1e18) <= (c*pc*10^db)*(cf*lf*util)
            let lhs = wide_mul(t as u128, (pb as u128) * 10u128.pow(dc) * 10u128.pow(18));
            let rhs = wide_mul((c as u128) * (pc as u128) * 10u128.pow(db), (cf as u128) * (lf as u128) * (util as u128));
            prop_assert!(lhs <= rhs);
        }

        #[test]
        fn max_util_headroom_respects_pool_check(
            supply in 1..=E18, liab_pct in 0u32..=100, max_util in 1u32..=9_900_000,
        ) {
            let liab = supply * i128::from(liab_pct) / 100;
            let h = max_util_headroom(supply, liab, max_util).unwrap();
            prop_assert!(h >= 0);
            if liab <= supply {
                // borrowing exactly `h` keeps utilization <= max_util, one more breaks it
                let util = |l: i128| if l == 0 { 0 } else if l >= supply { SCALAR_7 } else { (l * SCALAR_7 + supply - 1) / supply };
                if h > 0 { prop_assert!(util(liab + h) <= i128::from(max_util)); }
                if h > 0 || liab * SCALAR_7 <= supply * i128::from(max_util) {
                    prop_assert!(util(liab + h + 1) > i128::from(max_util) || liab + h + 1 >= supply);
                }
            }
        }

        // -- liquidity cap against the pool's own arithmetic --

        #[test]
        fn liquidity_cap_withdraw_never_reaches_full_utilization(
            b_supply in 1..=100_000_000_000_000_000_000i128,
            b_rate in E12..=6 * E12,
            d_rate in E12..=8 * E12,
            ratio_ppm in prop_oneof![0..=1_000_000i128, 999_000..=1_000_000i128, 999_990..=1_000_000i128],
        ) {
            let supply = b_supply * b_rate / E12;
            let d_supply = (supply * ratio_ppm / 1_000_000) * E12 / d_rate;
            let m = PoolModel { b_supply, b_rate, d_supply, d_rate };
            let cap = liquidity_cap(m.supply(), m.liab(), b_rate).unwrap();
            prop_assert!(cap >= 0 && cap <= m.supply());
            if cap > 0 {
                prop_assert!(m.withdraw_ok(cap), "cap {cap} breaks {m:?}");
            }
        }

        // -- unwind --

        #[test]
        fn unwind_invariants(
            c_pool in amt(E18), idle_c in amt(E18), d in amt(E18), idle_b in amt(E18),
            yield_w in amt(E18), pool_cap in amt(E18), min_residual in amt(E18 / 10),
            amount in 0..=2 * E18,
        ) {
            let c = c_pool + idle_c;
            let i = UnwindIn { amount, c, c_pool, idle_c, d, idle_b, yield_withdrawable: yield_w, pool_cap, min_residual };
            let o = unwind_split(&i).unwrap();
            let x = o.x_idle + o.x_pool;
            prop_assert!(o.x_idle >= 0 && o.x_pool >= 0 && o.r >= 0);
            prop_assert!(x <= amount && x <= c);
            prop_assert!(o.x_idle <= idle_c);
            prop_assert!(o.x_pool <= c_pool.min(pool_cap));
            prop_assert!(o.r <= d);
            prop_assert!(o.r <= idle_b + yield_w);
            if d == 0 { prop_assert_eq!(o.r, 0); }
            // C - x >= min_residual, unless the debt is gone or nothing is released
            prop_assert!(c - x >= min_residual || d - o.r == 0 || x == 0);
            // pro-rata: the repaid share is at least the released collateral share
            prop_assert!(o.r * c >= d * x);
            // idle-first delivery never worsens the pool-side debt/collateral ratio:
            // (D - r) / (C_pool - x_pool) <= D / C_pool
            if c_pool > 0 && c_pool - o.x_pool > 0 {
                prop_assert!((d - o.r) * c_pool <= d * (c_pool - o.x_pool));
            }
        }

        #[test]
        fn unwind_matches_plain_formula_when_unconstrained(
            c_pool in 1..=E18, idle_c in amt(E18), d in 1..=E18, amount in 1..=E18,
        ) {
            let c = c_pool + idle_c;
            let i = UnwindIn { amount, c, c_pool, idle_c, d, idle_b: d, yield_withdrawable: d, pool_cap: c_pool, min_residual: 0 };
            let o = unwind_split(&i).unwrap();
            let x = amount.min(c);
            prop_assert_eq!(o.x_idle + o.x_pool, x);
            prop_assert_eq!(o.r, (d * x + c - 1) / c);
        }

        // -- NAV --

        #[test]
        fn nav_matches_reference_model(
            c in 0..=1_000_000_000_000i128, y in 0..=1_000_000_000_000i128, d in 0..=1_000_000_000_000i128,
            band in 0u32..=50, h in 0u32..=1_000,
            pb in 1..=100_000_000i128, pc in 1..=100_000_000i128, db in 0u32..=8, dc in 0u32..=8,
        ) {
            let x = ctx(pb, pc, db, dc);
            if nav_in_band(y, d, band).unwrap() {
                // excluded value (b units) <= ceil(band * D / 1e4) <= band*D/1e4 + 1
                let diff = (y - d).abs();
                prop_assert!(diff * BPS < i128::from(band) * d + BPS);
            } else {
                let r = nav_priced(c, y, d, h, &x).unwrap();
                prop_assert!(r >= 0);
                // exact rational reference with common denominator `den`
                let den = pc * 10i128.pow(db) * BPS;
                let diff = y - d;
                let value = diff.abs() * pb * 10i128.pow(dc);
                let exact = if diff >= 0 {
                    c * den + value * (BPS - i128::from(h))
                } else {
                    c * den - value * BPS
                };
                if exact <= 0 {
                    prop_assert_eq!(r, 0);
                } else {
                    // never above the reference, never more than ~2 units below it
                    prop_assert!(r * den <= exact);
                    prop_assert!(exact - r * den < 2 * den);
                }
            }
        }

        #[test]
        fn nav_monotone_in_yield(
            c in 0..=E12, d in 0..=E12, y1 in 0..=E12, y2 in 0..=E12,
            h in 0u32..=1_000, pb in 1..=100_000_000i128, pc in 1..=100_000_000i128,
        ) {
            let x = ctx(pb, pc, 7, 7);
            let (lo, hi) = (y1.min(y2), y1.max(y2));
            prop_assert!(nav_priced(c, lo, d, h, &x).unwrap() <= nav_priced(c, hi, d, h, &x).unwrap());
        }

        // -- sweep --

        #[test]
        fn sweep_keeps_buffer_and_stays_in_band(
            y in 0..=E18, d in 0..=E18, band in 0u32..=50, buf_pct in 0u32..=100,
        ) {
            let buffer = band * buf_pct / 100; // buffer <= band
            let s = sweep_amount(y, d, buffer).unwrap();
            prop_assert!(s >= 0 && s <= y);
            let keep = (d * (BPS + i128::from(buffer)) + BPS - 1) / BPS;
            if s > 0 {
                prop_assert_eq!(y - s, keep);
                let threshold = (d * i128::from(band) + BPS - 1) / BPS;
                prop_assert!((y - s) - d <= threshold);
                prop_assert_eq!(nav_in_band(y - s, d, band), Ok(true));
            } else {
                prop_assert!(y <= keep);
            }
            prop_assert!(y - s >= d.min(y));
        }

        // -- params --

        #[test]
        fn validate_params_iff_bounds(
            util in 0u32..=10_000, buf in 0u32..=100, hc in 0u32..=2_000, band in 0u32..=100,
            age in 0u64..=200_000, minr in -10..=10i128,
        ) {
            let p = ParamsIn {
                borrow_utilization_bps: util, surplus_buffer_bps: buf, surplus_haircut_bps: hc,
                oracle_free_band_bps: band, oracle_max_age_secs: age, min_residual_collateral: minr,
            };
            let ok = util <= MAX_BORROW_UTILIZATION_BPS && hc <= MAX_SURPLUS_HAIRCUT_BPS
                && band <= MAX_ORACLE_FREE_BAND_BPS && buf <= band
                && age <= MAX_ORACLE_AGE_SECS && minr >= 0;
            prop_assert_eq!(validate_params(&p).is_ok(), ok);
        }
    }
}
