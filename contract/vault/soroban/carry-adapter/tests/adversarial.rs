//! Adversarial / failure-mode tests (carry plan step 8).
//!
//! Each test pins one hostile pool or adapter state to the exact expected
//! error variant or position delta: pool oracle staleness, health factor,
//! pool status gates, auctions, supply caps, utilization squeezes,
//! `min_collateral`, `max_util` clamping, bad debt, yield-pool illiquidity,
//! disabled reserves, pause and `realisation_pending`.

use soroban_sdk::{
    testutils::Ledger, token::Client as TokenClient, vec, Address, BytesN, Env, IntoVal, String,
};
use templar_soroban_carry_adapter::contract::{
    CarryAdapterContract, CarryAdapterContractClient, CarryError, Params,
};
use templar_soroban_carry_adapter::testutils::{
    default_reserve_metadata, CarryFixture, PoolClient, Request, RequestType, ReserveConfig,
};

/// Fork `PoolError` codes we assert on (blend-contracts-v2 errors.rs).
const INVALID_HF: u32 = 1205;
const INVALID_POOL_STATUS: u32 = 1206;
const EXCEEDED_SUPPLY_CAP: u32 = 1220;
const RESERVE_DISABLED: u32 = 1223;
const MIN_COLLATERAL_NOT_MET: u32 = 1224;

macro_rules! mock_auth {
    ($ctx:expr, $addr:expr, $fn_name:literal, ( $($arg:expr),* $(,)? )) => {{
        let args: soroban_sdk::Vec<soroban_sdk::Val> =
            ($($arg,)*).into_val(&$ctx.fx.env);
        $ctx.fx.env.mock_auths(&[soroban_sdk::testutils::MockAuth {
            address: $addr,
            invoke: &soroban_sdk::testutils::MockAuthInvoke {
                contract: &$ctx.client.address,
                fn_name: $fn_name,
                args: args.into(),
                sub_invokes: &[],
            },
        }]);
    }};
}

struct Ctx<'a> {
    fx: &'a CarryFixture<'a>,
    client: CarryAdapterContractClient<'a>,
    admin: Address,
    vault: Address,
}

impl<'a> Ctx<'a> {
    fn new(fx: &'a CarryFixture<'a>) -> Self {
        Self::new_pools(
            fx,
            fx.pool.address.clone(),
            fx.yield_pool.address.clone(),
            &Ctx::default_params(),
        )
    }

    fn new_pools(
        fx: &'a CarryFixture<'a>,
        pool: Address,
        yield_pool: Address,
        params: &Params,
    ) -> Self {
        let admin = fx.admin.clone();
        let vault = fx.whale.clone();
        let id = fx.env.register(
            CarryAdapterContract,
            (
                admin.clone(),
                vault.clone(),
                pool,
                yield_pool,
                fx.collateral_token.clone(),
                fx.borrow_token.clone(),
                fx.whale.clone(),
                params.borrow_utilization_bps,
                params.surplus_buffer_bps,
                params.surplus_haircut_bps,
                params.oracle_free_band_bps,
                params.oracle_max_age_secs,
                params.min_residual_collateral,
            ),
        );
        Self {
            fx,
            client: CarryAdapterContractClient::new(&fx.env, &id),
            admin,
            vault,
        }
    }

    /// The fixture's collateral pool reserve configs.
    fn pool_configs() -> (ReserveConfig, ReserveConfig) {
        let mut collateral = default_reserve_metadata();
        collateral.c_factor = 9_000_000;
        collateral.l_factor = 9_500_000;
        collateral.supply_cap = i128::MAX;
        (collateral, default_reserve_metadata())
    }

    fn default_params() -> Params {
        Params {
            borrow_utilization_bps: 8_000,
            surplus_buffer_bps: 10,
            surplus_haircut_bps: 500,
            oracle_free_band_bps: 50,
            oracle_max_age_secs: 86_400,
            min_residual_collateral: 0,
        }
    }

    fn mint_collateral(&self, amount: i128) {
        CarryFixture::mint(
            &self.fx.env,
            &self.fx.collateral_token,
            &self.client.address,
            amount,
        );
    }

    fn mint_idle_b(&self, amount: i128) {
        CarryFixture::mint(
            &self.fx.env,
            &self.fx.borrow_token,
            &self.client.address,
            amount,
        );
    }

    /// `supply` as the vault, with vault auth registered for exactly this call.
    fn supply(&self, amount: i128) {
        mock_auth!(
            self,
            &self.vault,
            "supply",
            (&self.vault, &self.fx.collateral_token, amount)
        );
        self.client
            .supply(&self.vault, &self.fx.collateral_token, &amount);
    }

    fn deleverage(&self) {
        mock_auth!(self, &self.admin, "deleverage", (&self.admin,));
        self.client.deleverage(&self.admin);
    }

    fn deleverage_try(&self) -> Result<Result<(), soroban_sdk::Error>, soroban_sdk::Error> {
        mock_auth!(self, &self.admin, "deleverage", (&self.admin,));
        let res = self.client.try_deleverage(&self.admin);
        map_nested(res)
    }

    fn progress(&self, amount: i128) -> i128 {
        mock_auth!(
            self,
            &self.vault,
            "progress_withdrawal",
            (&self.vault, &self.fx.collateral_token, amount)
        );
        self.client
            .progress_withdrawal(&self.vault, &self.fx.collateral_token, &amount)
    }

    fn progress_try(
        &self,
        amount: i128,
    ) -> Result<Result<i128, soroban_sdk::Error>, soroban_sdk::Error> {
        mock_auth!(
            self,
            &self.vault,
            "progress_withdrawal",
            (&self.vault, &self.fx.collateral_token, amount)
        );
        let res =
            self.client
                .try_progress_withdrawal(&self.vault, &self.fx.collateral_token, &amount);
        map_nested(res)
    }

    /// Mints idle b so deleverage/sweep funding is guaranteed to cover D.
    fn top_up_idle_b_to_cover_debt(&self) {
        let pos = self.client.position();
        if pos.d > pos.y {
            self.mint_idle_b(pos.d - pos.y);
        }
    }

    /// Deploys an extra pool through the pinned factory with the given
    /// reserve configs (collateral at index 0, borrow at index 1) and
    /// activates it via the mock backstop's genuine threshold check.
    fn build_pool(
        &self,
        tag: &str,
        collateral_cfg: ReserveConfig,
        borrow_cfg: ReserveConfig,
        whale_borrow_supply: i128,
    ) -> PoolClient<'a> {
        let env = &self.fx.env;
        env.mock_all_auths();
        // Unique salt per pool: the factory derives the address from
        // (admin, salt), so a constant salt would collide across calls.
        let mut salt_bytes = [0u8; 32];
        for (i, b) in tag.as_bytes().iter().enumerate() {
            salt_bytes[i] = *b;
        }
        let salt = BytesN::from_array(env, &salt_bytes);
        let addr = self.fx.pool_factory.deploy(
            &self.admin,
            &String::from_str(env, tag),
            &salt,
            &self.fx.oracle.address().clone(),
            &0u32,
            &10u32,
            &1_0000000,
        );
        let pool = PoolClient::new(env, &addr);
        pool.queue_set_reserve(&self.fx.collateral_token, &collateral_cfg);
        pool.set_reserve(&self.fx.collateral_token);
        pool.queue_set_reserve(&self.fx.borrow_token, &borrow_cfg);
        pool.set_reserve(&self.fx.borrow_token);

        // Activate through the harness helper (mock backstop data clears
        // the threshold via the genuine threshold/q4w check, no q4w).
        self.fx.seed_extra_pool(&pool);

        // Seed borrow liquidity so supply legs can borrow.
        CarryFixture::mint(
            env,
            &self.fx.borrow_token,
            &self.fx.whale,
            whale_borrow_supply,
        );
        let request = Request {
            request_type: RequestType::Supply as u32,
            address: self.fx.borrow_token.clone(),
            amount: whale_borrow_supply,
        };
        pool.submit(
            &self.fx.whale,
            &self.fx.whale,
            &self.fx.whale,
            &vec![env, request],
        );
        pool
    }

    /// Disables `token` on `pool` through the genuine disable-only reserve
    /// path (timelock honoured).
    fn disable_reserve(&self, pool: &PoolClient, token: &Address) {
        // queue/set_reserve are admin-gated on the fork pool.
        self.fx.env.mock_all_auths();
        let mut cfg = self.fx.reserve(pool, token).config;
        cfg.enabled = false;
        pool.queue_set_reserve(token, &cfg);
        self.fx.advance_time(7 * 24 * 3600 + 1);
        pool.set_reserve(token);
        assert!(
            !self.fx.reserve(pool, token).config.enabled,
            "reserve disabled via timelocked path"
        );
    }

    fn third_party_supply(
        &self,
        pool: &PoolClient,
        token: &Address,
        amount: i128,
        as_collateral: bool,
    ) {
        let request = Request {
            request_type: if as_collateral {
                RequestType::SupplyCollateral as u32
            } else {
                RequestType::Supply as u32
            },
            address: token.clone(),
            amount,
        };
        pool.submit(
            &self.fx.third_party,
            &self.fx.third_party,
            &self.fx.third_party,
            &vec![&self.fx.env, request],
        );
    }

    fn third_party_borrow(&self, pool: &PoolClient, token: &Address, amount: i128) {
        let request = Request {
            request_type: RequestType::Borrow as u32,
            address: token.clone(),
            amount,
        };
        pool.submit(
            &self.fx.third_party,
            &self.fx.third_party,
            &self.fx.third_party,
            &vec![&self.fx.env, request],
        );
    }

    fn whale_withdraw(&self, pool: &PoolClient, token: &Address, amount: i128) {
        let request = Request {
            request_type: RequestType::WithdrawCollateral as u32,
            address: token.clone(),
            amount,
        };
        pool.submit(
            &self.fx.whale,
            &self.fx.whale,
            &self.fx.whale,
            &vec![&self.fx.env, request],
        );
    }
}

/// Contract error code carried by any failed `try_*` client call, if any.
/// `Err(e)` (unwind / host abort) and `Ok(Err(e))` (adapter or pool
/// contract error) shapes produced by `map_nested` on generated clients.
fn code_of<T>(res: &Result<Result<T, soroban_sdk::Error>, soroban_sdk::Error>) -> Option<u32> {
    let err = match res {
        Err(e) => *e,
        Ok(Err(e)) => *e,
        Ok(Ok(_)) => return None,
    };
    if err.is_type(soroban_sdk::xdr::ScErrorType::Contract) {
        Some(err.get_code())
    } else {
        None
    }
}

/// Normalizes the generated client's nested error shape
/// `Result<Result<T, E>, Result<CarryError, InvokeError>>` to
/// `Result<Result<T, Error>, Error>` so one assertion helper covers
/// adapter errors, pool errors and host aborts uniformly.
fn map_nested<T, E>(
    res: Result<Result<T, E>, Result<CarryError, soroban_sdk::InvokeError>>,
) -> Result<Result<T, soroban_sdk::Error>, soroban_sdk::Error>
where
    E: Into<soroban_sdk::Error>,
{
    res.map(|inner| inner.map_err(|e| e.into()))
        .map_err(|outer| match outer {
            Ok(ce) => ce.into(),
            Err(ie) => match ie {
                soroban_sdk::InvokeError::Contract(code) => {
                    soroban_sdk::Error::from_contract_error(code)
                }
                soroban_sdk::InvokeError::Abort => soroban_sdk::Error::from_contract_error(0xffff),
            },
        })
}

fn assert_pool_error<T>(
    res: &Result<Result<T, soroban_sdk::Error>, soroban_sdk::Error>,
    code: u32,
    label: &str,
) {
    assert_eq!(
        code_of(res),
        Some(code),
        "{label}: expected pool/contract error {code}, got {:?}",
        res.as_ref().err().unwrap()
    );
}

/// `total supply - total liabilities` of a reserve (underlying units).
fn reserve_free(ctx: &Ctx, pool: &PoolClient, token: &Address) -> i128 {
    let r = ctx.fx.reserve(pool, token);
    let supply = r.data.b_supply * r.data.b_rate / 1_000_000_000_000;
    let liab = (r.data.d_supply * r.data.d_rate + 1_000_000_000_000 - 1) / 1_000_000_000_000;
    supply - liab
}

// ---------------------------------------------------------------------------
// Pool oracle
// ---------------------------------------------------------------------------

#[test]
fn pool_oracle_stale_partial_unwind_reverts_full_repay_passes() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.supply(10_0000000);
    let p0 = ctx.client.position();
    assert!(p0.d > 0 && p0.y_pool > 0);

    // Stale feed: advance the ledger past the oracle freshness threshold
    // without re-stamping prices. The adapter accrues debt interest on
    // reads, so the revert baseline is captured at the NEW timestamp.
    fx.env
        .ledger()
        .set_timestamp(fx.env.ledger().timestamp() + 1_000);
    let p0 = ctx.client.position();

    // Negative carry accrued across the 1000s advance: the yield leg is
    // short of the debt by the borrow-reserve accrual. Top up idle b so
    // the full-exit plan funds in full (x = C, r = D) and the pool batch
    // ends with an empty position, skipping the oracle consult.
    ctx.top_up_idle_b_to_cover_debt();

    // Partial unwind: the WithdrawCollateral triggers the pool health check,
    // which reads the (stale) oracle and traps -> adapter batch reverts.
    let res = ctx.progress_try(p0.c / 2);
    assert!(
        res.is_err(),
        "partial unwind must revert with a stale oracle"
    );
    let p1 = ctx.client.position();
    assert_eq!(p1.c_pool, p0.c_pool, "collateral unchanged");
    assert_eq!(p1.d, p0.d, "debt unchanged");
    assert_eq!(p1.y_pool, p0.y_pool, "yield leg unchanged");

    // Full repay: liabilities hit zero, the health check is skipped, the
    // batch passes without consulting the oracle.
    let delivered = ctx.progress(p0.c + 1);
    assert_eq!(delivered, p0.c);
    let p2 = ctx.client.position();
    assert_eq!(p2.d, 0);
    assert_eq!(p2.c_pool, 0);
    assert_eq!(p2.y_pool, 0);
}

// ---------------------------------------------------------------------------
// Health factor / deleverage / auctions
// ---------------------------------------------------------------------------

/// Creates a user liquidation auction on the adapter (returns the percent
/// used). The loop finds the largest legal percent; panics if none exists.
fn force_liq_auction(ctx: &Ctx) -> u32 {
    for percent in (1u32..=100u32).rev() {
        let res = ctx.fx.pool.try_new_auction(
            &0u32,
            &ctx.client.address,
            &vec![&ctx.fx.env, ctx.fx.borrow_token.clone()],
            &vec![&ctx.fx.env, ctx.fx.collateral_token.clone()],
            &percent,
        );
        if res.is_ok() {
            return percent;
        }
    }
    panic!("could not force a liquidation auction (position not liquidatable?)");
}

#[test]
fn hf_below_threshold_unwind_reverts_deleverage_then_unwind_passes() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.supply(10_0000000);
    let p0 = ctx.client.position();

    // 25% price drop: collateral base 67_5000000 < liability base 72_0000000
    // -> HF < 1 with debt remaining -> unwind's health check reverts.
    ctx.fx
        .set_oracle_price(&ctx.fx.collateral_token, 75_000_000_000);
    let res = ctx.progress_try(p0.c / 2);
    assert_pool_error(&res, INVALID_HF, "partial unwind at HF<1");
    let p1 = ctx.client.position();
    assert_eq!(p1.c_pool, p0.c_pool);
    assert_eq!(p1.d, p0.d);

    // deleverage (no auction present here) repays in full, then unwind
    // passes (D == 0 -> health check skipped).
    ctx.top_up_idle_b_to_cover_debt();
    ctx.deleverage();
    assert_eq!(ctx.client.position().d, 0, "deleverage cleared debt");
    assert_eq!(
        ctx.client.position().c_pool,
        p0.c_pool,
        "deleverage releases no collateral"
    );

    let delivered = ctx.progress(p0.c + 1);
    assert_eq!(delivered, p0.c);
    assert_eq!(ctx.client.position().c_pool, 0);
}

#[test]
fn auction_in_progress_then_deleverage_with_delete_auction_then_unwind() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.supply(10_0000000);
    let p0 = ctx.client.position();

    // Liquidatable position -> auction opens on the adapter.
    ctx.fx
        .set_oracle_price(&ctx.fx.collateral_token, 75_000_000_000);
    let percent = force_liq_auction(&ctx);
    assert!(percent > 0);

    // Unwind reverts: HF < 1 on the post-batch position (the batch includes
    // the delete-auction, but the remaining debt is still unhealthy).
    let res = ctx.progress_try(p0.c / 2);
    assert_pool_error(&res, INVALID_HF, "partial unwind with open auction");

    // deleverage repays in full and deletes the auction.
    ctx.top_up_idle_b_to_cover_debt();
    ctx.deleverage();
    assert_eq!(ctx.client.position().d, 0, "auction debt repaid");
    assert!(
        ctx.fx
            .pool
            .try_get_auction(&0u32, &ctx.client.address)
            .is_err(),
        "delete-auction request removed the auction"
    );

    // unwind passes afterwards.
    let delivered = ctx.progress(p0.c + 1);
    assert_eq!(delivered, p0.c);
    assert_eq!(ctx.client.position().c_pool, 0);
}

#[test]
fn auction_status_gt1_blocks_deleverage_recovery_at_status_0() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.supply(10_0000000);

    // Open an auction, then push the pool On-Ice (status 2).
    ctx.fx
        .set_oracle_price(&ctx.fx.collateral_token, 75_000_000_000);
    force_liq_auction(&ctx);
    fx.env.mock_all_auths(); // set_status is admin-gated
    ctx.fx.pool.set_status(&2u32);

    // deleverage's batch includes the delete-auction request; at status > 1
    // the pool rejects request type 9 outright.
    let res = ctx.deleverage_try();
    assert_pool_error(&res, INVALID_POOL_STATUS, "deleverage batch at status 2");
    assert!(ctx.client.position().d > 0, "debt intact");
    assert!(
        ctx.fx
            .pool
            .try_get_auction(&0u32, &ctx.client.address)
            .is_ok(),
        "auction still open"
    );

    // Recover: back to Active, deleverage deletes the auction and repays.
    fx.env.mock_all_auths(); // deleverage_try's mock_auths cleared allow-all
    ctx.fx.pool.set_status(&0u32);
    ctx.top_up_idle_b_to_cover_debt();
    ctx.deleverage();
    assert_eq!(ctx.client.position().d, 0);
    assert!(ctx
        .fx
        .pool
        .try_get_auction(&0u32, &ctx.client.address)
        .is_err());
}

#[test]
fn auction_open_healthy_position_unwind_deletes_it_and_passes() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.supply(10_0000000);
    let p0 = ctx.client.position();

    // Open an auction at a liquidatable price, then restore the feed: the
    // position is healthy again but the auction still blocks submits.
    ctx.fx
        .set_oracle_price(&ctx.fx.collateral_token, 75_000_000_000);
    force_liq_auction(&ctx);
    ctx.fx
        .set_oracle_price(&ctx.fx.collateral_token, 100_000_000_000);

    // The unwind batch carries delete-auction -> the auction is deleted and
    // the restored HF lets the release through.
    let delivered = ctx.progress(p0.c / 2);
    assert!(delivered > 0 && delivered <= p0.c / 2 + 1);
    assert!(
        ctx.fx
            .pool
            .try_get_auction(&0u32, &ctx.client.address)
            .is_err(),
        "auction deleted by the unwind batch"
    );
    assert!(
        ctx.client.position().c_pool < p0.c_pool,
        "collateral released"
    );
}

#[test]
fn supply_cap_hit_reverts_exceeded_supply_cap() {
    // collateral reserve cap = 54 units; whale holds 50, adapter supplies
    // 4 -> exactly at cap.
    let (collateral_cfg, borrow_cfg) = Ctx::pool_configs();
    let mut capped = collateral_cfg.clone();
    capped.supply_cap = 54_0000000;
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx0 = Ctx::new(&fx);
    let pool = ctx0.build_pool("CapPool", capped, borrow_cfg.clone(), 5_000_000_000_000);
    let yield_pool = ctx0.build_pool(
        "CapYield",
        borrow_cfg.clone(),
        borrow_cfg.clone(),
        5_000_000_000_000,
    );
    let ctx = Ctx::new_pools(
        &fx,
        pool.address.clone(),
        yield_pool.address.clone(),
        &Ctx::default_params(),
    );

    // The whale posts 50 BTC collateral into the capped pool first so the
    // adapter's 4 BTC supply lands exactly on the 54-unit cap.
    CarryFixture::mint(&fx.env, &fx.collateral_token, &fx.whale, 50_0000000);
    fx.env.mock_all_auths();
    pool.submit(
        &fx.whale,
        &fx.whale,
        &fx.whale,
        &vec![
            &env,
            Request {
                request_type: RequestType::SupplyCollateral as u32,
                address: fx.collateral_token.clone(),
                amount: 50_0000000,
            },
        ],
    );

    ctx.mint_collateral(4_0000000);
    ctx.supply(4_0000000);
    assert_eq!(
        ctx.client.position().c_pool,
        4_0000000,
        "supply at the cap succeeds"
    );
    assert!(
        ctx.client.position().d > 0,
        "borrow leg opened under the cap"
    );

    // One more stroop exceeds the cap: the pool reverts and the adapter
    // position is unchanged (the failed sub-call rolls the whole batch
    // back).
    ctx.mint_collateral(1);
    let res = map_nested(
        ctx.client
            .try_supply(&ctx.vault, &ctx.fx.collateral_token, &1i128),
    );
    assert_pool_error(&res, EXCEEDED_SUPPLY_CAP, "supply past cap");
    assert_eq!(
        ctx.client.position().c_pool,
        4_0000000,
        "position unchanged"
    );
}

// ---------------------------------------------------------------------------
// Pool status
// ---------------------------------------------------------------------------

#[test]
fn pool_status_on_ice_blocks_borrow_unwind_ok() {
    for status in [2u32, 3u32] {
        let env = Env::default();
        let fx = CarryFixture::new(&env);
        let ctx = Ctx::new(&fx);
        ctx.mint_collateral(10_0000000);
        ctx.supply(10_0000000);
        let p0 = ctx.client.position();
        assert!(p0.d > 0, "fixture state has a borrow leg");

        fx.env.mock_all_auths(); // set_status is admin-gated
        ctx.fx.pool.set_status(&status);

        // supply's second batch is a Borrow -> blocked at status > 1.
        ctx.mint_collateral(1_0000000); // retry supply collateral
        let res = map_nested(ctx.client.try_supply(
            &ctx.vault,
            &ctx.fx.collateral_token,
            &1_0000000,
        ));
        assert_pool_error(
            &res,
            INVALID_POOL_STATUS,
            &format!("supply at status {status}"),
        );
        let p1 = ctx.client.position();
        assert_eq!(p1.c_pool, p0.c_pool, "supply reverted whole call");
        assert_eq!(p1.d, p0.d);

        // unwind (Repay + WithdrawCollateral) is allowed.
        let delivered = ctx.progress(p0.c / 2);
        assert!(delivered > 0);
        assert!(ctx.client.position().d < p0.d, "debt repaid");
    }
}

#[test]
fn pool_frozen_status_4_supply_reverts_unwind_passes() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.supply(10_0000000);
    let p0 = ctx.client.position();

    fx.env.mock_all_auths(); // set_status is admin-gated
    ctx.fx.pool.set_status(&4u32);

    let res = map_nested(
        ctx.client
            .try_supply(&ctx.vault, &ctx.fx.collateral_token, &1_0000000),
    );
    assert_pool_error(&res, INVALID_POOL_STATUS, "supply on frozen pool");
    assert_eq!(ctx.client.position().c_pool, p0.c_pool);

    let delivered = ctx.progress(p0.c + 1);
    assert_eq!(delivered, p0.c, "unwind passes on a frozen pool");
    assert_eq!(ctx.client.position().c_pool, 0);
}

#[test]
fn yield_pool_frozen_supply_reverts_unwind_passes() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.supply(10_0000000);
    let p0 = ctx.client.position();

    fx.env.mock_all_auths(); // set_status is admin-gated
    ctx.fx.yield_pool.set_status(&4u32);

    // The collateral + borrow legs succeed; the yield-leg Supply reverts
    // the whole adapter call -> adapter state unchanged.
    ctx.mint_collateral(1_0000000); // retry supply collateral
    let res = map_nested(
        ctx.client
            .try_supply(&ctx.vault, &ctx.fx.collateral_token, &1_0000000),
    );
    assert_pool_error(&res, INVALID_POOL_STATUS, "yield leg on frozen yield pool");
    let p1 = ctx.client.position();
    assert_eq!(p1.c_pool, p0.c_pool);
    assert_eq!(p1.d, p0.d);
    assert_eq!(p1.y_pool, p0.y_pool);

    // Withdraw from the frozen yield pool is allowed; unwind passes.
    let delivered = ctx.progress(p0.c / 2);
    assert!(delivered > 0);
    assert!(ctx.client.position().y_pool < p0.y_pool);
}

// ---------------------------------------------------------------------------
// Custom pools for reserve-level state
// ---------------------------------------------------------------------------

#[test]
fn borrow_max_util_clamp_caps_debt() {
    // borrow reserve max_util = 5% of 500k whale supply = 25k units headroom.
    let (_, borrow_cfg) = Ctx::pool_configs();
    let mut clamped = borrow_cfg.clone();
    clamped.max_util = 500_000;
    clamped.util = 100_000;
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx0 = Ctx::new(&fx);
    let pool = ctx0.build_pool(
        "ClampPool",
        Ctx::pool_configs().0,
        clamped.clone(),
        5_000_000_000_000,
    );
    let yield_pool = ctx0.build_pool(
        "ClampYield",
        clamped.clone(),
        clamped.clone(),
        5_000_000_000_000,
    );
    let ctx = Ctx::new_pools(
        &fx,
        pool.address.clone(),
        yield_pool.address.clone(),
        &Ctx::default_params(),
    );

    ctx.mint_collateral(10_0000000);
    ctx.supply(10_0000000);

    let pos = ctx.client.position();
    // Unclamped target is 54_0000000 (80% util on 10 BTC at 90/95);
    // headroom is floor(500k * 0.05) = 25_000_0000000 (25k units at 7 dec).
    assert!(pos.d > 0, "borrow opened");
    assert!(
        pos.d <= 250_000_000_000 + 2,
        "debt clamped to max_util headroom: {}",
        pos.d
    );
    assert_eq!(
        pos.y_pool, pos.d,
        "clamped borrow fully deployed to the yield leg"
    );
    let r = ctx.fx.reserve(&pool, &ctx.fx.borrow_token);
    let supply = r.data.b_supply * r.data.b_rate / 1_000_000_000_000;
    // utilization = 1e7 - free*1e7/supply  (free from reserve_free)
    let free = reserve_free(&ctx, &pool, &ctx.fx.borrow_token);
    let util = 10_000_000 - free * 10_000_000 / supply;
    assert!(
        util <= clamped.max_util as i128,
        "utilization under max_util"
    );
}

// ---------------------------------------------------------------------------
// Utilization / residual / liquidity squeezes
// ---------------------------------------------------------------------------

#[test]
fn collateral_reserve_100pct_utilization_partial_release_via_liquidity_cap_no_panic() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.supply(10_0000000);
    let p0 = ctx.client.position();

    // Pin the collateral reserve near its health-factor boundary: a third
    // party borrows against its own collateral right up to l_factor. The
    // fork pool rejects flash loans outright (`flash_loan` -> BadRequest)
    // and aborts withdrawals beyond a user's b-token balance with #1217,
    // so the borrow is capped at the HF limit and each user's withdrawal
    // at their own balance - the reserve's free liquidity can never drop
    // below the adapter's own collateral plus every user's l_factor
    // headroom (>= p0.c_pool here).
    fx.env.mock_all_auths(); // third-party submits need auth
    CarryFixture::mint(
        &fx.env,
        &ctx.fx.collateral_token,
        &ctx.fx.third_party,
        200_0000000,
    );
    ctx.third_party_supply(&ctx.fx.pool, &ctx.fx.collateral_token, 150_0000000, true);
    // HF-valid borrow: 150 * c_factor(0.9) * l_factor(0.95) = 128.25 max.
    ctx.third_party_borrow(&ctx.fx.pool, &ctx.fx.collateral_token, 128_0000000);

    // The whale withdraws its full 50-unit collateral balance (requesting
    // more would abort with #1217), leaving 160 BTC supplied against
    // 128 BTC of liabilities: ~80% collateral-reserve utilization.
    let free_before = reserve_free(&ctx, &ctx.fx.pool, &ctx.fx.collateral_token);
    assert!(free_before >= p0.c_pool, "reserve has room for the squeeze");
    ctx.whale_withdraw(&ctx.fx.pool, &ctx.fx.collateral_token, 50_0000000);
    let free_after = reserve_free(&ctx, &ctx.fx.pool, &ctx.fx.collateral_token);
    assert!(
        free_after <= 32_0000000 + 10,
        "reserve squeezed to ~80% utilization"
    );

    // The adapter's release request stays within the free floor its own
    // collateral contributes, so it completes at full size: no panic, no
    // ZeroWithdrawal, no revert. (The adapter-side liquidity cap's partial
    // release path is pinned by yield_pool_illiquidity_partial_release.)
    let delivered = ctx.progress(p0.c + 1);
    assert!(delivered > 0, "release funded, not ZeroWithdrawal");
    assert_eq!(delivered, p0.c, "release completes at full size");
    assert_eq!(
        ctx.client.position().c_pool,
        0,
        "collateral released in full"
    );
}

#[test]
fn min_collateral_residual_pool_reverts_then_adapter_clamps() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.supply(10_0000000);
    let p0 = ctx.client.position();

    // Request all but 500 stroops: the leftover collateral_base ($0.45)
    // falls under the pool's min_collateral ($1) -> pool reverts the batch.
    let res = ctx.progress_try(p0.c - 500);
    assert_pool_error(&res, MIN_COLLATERAL_NOT_MET, "min_collateral breach");
    assert_eq!(ctx.client.position().c_pool, p0.c_pool, "unchanged");
    // Adapter-side clamp: min_residual_collateral = 10_000 stroops leaves a
    // $10.00 collateral base (>= $1 pool minimum) and releases the rest.
    // The same over-large request from part 1 now gets clamped by the
    // adapter (x = C - min_residual) instead of reaching the pool's
    // min_collateral revert; debt remains so the residual applies.
    fx.env.mock_all_auths(); // set_params is admin-gated
    ctx.client.set_params(
        &ctx.admin,
        &Params {
            min_residual_collateral: 10_000,
            ..Ctx::default_params()
        },
    );
    let delivered = ctx.progress(p0.c - 500);
    assert_eq!(delivered, p0.c - 10_000, "clamped to C - min_residual");
    assert!(ctx.client.position().c_pool >= 9_000, "residual kept");
}

// ---------------------------------------------------------------------------
// Bad debt and yield-pool illiquidity
// ---------------------------------------------------------------------------

#[test]
fn bad_debt_price_crash_nav_unchanged_hf_blocks_partial_full_exit_passes() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.supply(10_0000000);
    let p0 = ctx.client.position();

    // Crash the collateral price; the carry position is price-neutral in
    // NAV terms (Y == D, band applies): partial unwinds revert on HF, the
    // full exit repays everything and passes.
    ctx.fx
        .set_oracle_price(&ctx.fx.collateral_token, 30_000_000_000);
    let nav = ctx.client.total_assets(&ctx.fx.collateral_token);
    assert_eq!(nav, p0.c, "NAV unchanged by the collateral crash");

    let res = ctx.progress_try(p0.c / 2);
    assert_pool_error(&res, INVALID_HF, "partial unwind under bad debt");
    assert_eq!(ctx.client.position().c_pool, p0.c_pool, "unchanged");

    let delivered = ctx.progress(p0.c + 1);
    assert_eq!(delivered, p0.c, "full exit passes");
    let p2 = ctx.client.position();
    assert_eq!(p2.d, 0);
    assert_eq!(p2.c_pool, 0);
    assert_eq!(p2.y_pool, 0);
}

#[test]
fn bad_debt_negative_carry_nav_deducts_shortfall_unwind_funds_partially() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.supply(10_0000000);

    fx.env.mock_all_auths(); // third-party submits need auth
                             // 365 days of accrual: the pool borrow reserve accrues (third-party
                             // utilization), the yield leg earns ~0 -> D grows past Y.
                             // TP must collateralize before borrowing (Blend #1205 otherwise):
                             // post TP's 0.1 BTC collateral, then borrow 100 USDC.
    ctx.third_party_supply(&ctx.fx.pool, &ctx.fx.collateral_token, 1_0000000, true);
    ctx.third_party_supply(&ctx.fx.pool, &ctx.fx.borrow_token, 100_0000000, false);
    CarryFixture::mint(
        &fx.env,
        &ctx.fx.borrow_token,
        &ctx.fx.third_party,
        10_0000000,
    );
    ctx.third_party_borrow(&ctx.fx.pool, &ctx.fx.borrow_token, 10_0000000);
    ctx.fx.advance_time(365 * 24 * 3600);

    let p1 = ctx.client.position();
    assert!(p1.d > p1.y, "shortfall accrued: D {} > Y {}", p1.d, p1.y);

    // NAV outside the band is priced: the shortfall deducts from C.
    let nav = ctx.client.total_assets(&ctx.fx.collateral_token);
    assert!(
        nav < p1.c,
        "NAV deducts the shortfall (nav {nav} < C {})",
        p1.c
    );

    // Unwind is funded only up to Y: the release shrinks pro-rata.
    let expected = p1.c * p1.y / p1.d;
    let delivered = ctx.progress(p1.c + 1);
    assert!(
        delivered <= expected + 2 && delivered >= expected - 2,
        "delivered {delivered} ~= floor(C*Y/D) = {expected}"
    );
    assert!(ctx.client.position().d > 0, "debt tail remains (bad debt)");
}

#[test]
fn yield_pool_illiquidity_partial_release_and_zero_withdrawal() {
    // yield pool with the collateral reserve enabled too, so a third party
    // can borrow the borrow asset and drain pool liquidity.
    let (collateral_cfg, borrow_cfg) = Ctx::pool_configs();
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx0 = Ctx::new(&fx);
    let pool = ctx0.build_pool(
        "IllqPool",
        collateral_cfg.clone(),
        borrow_cfg.clone(),
        5_000_000_000_000,
    );
    let yield_pool = ctx0.build_pool("IllqYield", collateral_cfg, borrow_cfg, 5_000_000_000_000);
    let ctx = Ctx::new_pools(
        &fx,
        pool.address.clone(),
        yield_pool.address.clone(),
        &Ctx::default_params(),
    );

    ctx.mint_collateral(10_0000000);
    ctx.supply(10_0000000);
    let p0 = ctx.client.position();
    assert!(p0.d > 0);

    // Drain the yield pool: third party supplies collateral and borrows
    // almost the full reserve.
    CarryFixture::mint(
        &fx.env,
        &ctx.fx.collateral_token,
        &ctx.fx.third_party,
        200_0000000,
    );
    ctx.third_party_supply(&yield_pool, &ctx.fx.collateral_token, 150_0000000, true);
    let free = reserve_free(&ctx, &yield_pool, &ctx.fx.borrow_token);
    let drain = free * 94 / 100;
    ctx.third_party_borrow(&yield_pool, &ctx.fx.borrow_token, drain);
    let free_after = reserve_free(&ctx, &yield_pool, &ctx.fx.borrow_token);
    assert!(free_after < free / 10, "yield pool drained to illiquid");

    // Partial unwind: funding f < r shrinks the release; no panic.
    let delivered = ctx.progress(p0.c + 1);
    assert!(
        delivered > 0 && delivered < p0.c,
        "partial release under illiquidity"
    );
    assert!(ctx.client.position().d > 0);

    // With the yield pool down to ~1 unit of free liquidity, a second full
    // unwind is (near-)unfunded: it either returns a tiny partial or the
    // ZeroWithdrawal error - both non-panicking outcomes.
    let c1 = ctx.client.position().c;
    let second = ctx.progress_try(c1 + 1);
    match second {
        Ok(Ok(v)) => assert!(v > 0 && v < delivered, "second release smaller: {v}"),
        err => {
            assert!(
                code_of(&err) == Some(CarryError::ZeroWithdrawal as u32),
                "expected ZeroWithdrawal, got {:?}",
                err.err()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Reserve disabled
// ---------------------------------------------------------------------------

#[test]
fn collateral_reserve_disabled_supply_reverts_unwind_passes() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.supply(10_0000000);
    let p0 = ctx.client.position();

    ctx.disable_reserve(&ctx.fx.pool, &ctx.fx.collateral_token);

    ctx.mint_collateral(1_0000000); // retry supply collateral
    let res = map_nested(
        ctx.client
            .try_supply(&ctx.vault, &ctx.fx.collateral_token, &1_0000000),
    );
    assert_pool_error(
        &res,
        RESERVE_DISABLED,
        "supply on disabled collateral reserve",
    );
    assert_eq!(ctx.client.position().c_pool, p0.c_pool);

    let delivered = ctx.progress(p0.c / 2);
    assert!(
        delivered > 0,
        "unwind (withdraw/repay) allowed on disabled reserve"
    );
}

#[test]
fn borrow_reserve_disabled_supply_reverts_unwind_passes() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.supply(10_0000000);
    let p0 = ctx.client.position();

    ctx.disable_reserve(&ctx.fx.pool, &ctx.fx.borrow_token);

    // Collateral-only supply would pass, but the adapter's batch is
    // SupplyCollateral + Borrow -> the whole call reverts.
    ctx.mint_collateral(1_0000000); // retry supply collateral
    let res = map_nested(
        ctx.client
            .try_supply(&ctx.vault, &ctx.fx.collateral_token, &1_0000000),
    );
    assert_pool_error(&res, RESERVE_DISABLED, "borrow on disabled reserve");
    assert_eq!(ctx.client.position().c_pool, p0.c_pool);

    let delivered = ctx.progress(p0.c / 2);
    assert!(
        delivered > 0,
        "repay/withdraw allowed on disabled borrow reserve"
    );
}

#[test]
fn yield_reserve_disabled_supply_reverts_unwind_passes() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.supply(10_0000000);
    let _p0 = ctx.client.position();

    ctx.disable_reserve(&ctx.fx.yield_pool, &ctx.fx.borrow_token);

    // disable_reserve advances the ledger; borrow debt accrues on the new
    // clock. Re-snapshot the baseline so the revert check compares debt
    // valued at the same d_rate.
    let p0 = ctx.client.position();

    // pool legs succeed, the yield-leg Supply reverts the adapter call.
    ctx.mint_collateral(1_0000000); // retry supply collateral
    let res = map_nested(
        ctx.client
            .try_supply(&ctx.vault, &ctx.fx.collateral_token, &1_0000000),
    );
    assert_pool_error(&res, RESERVE_DISABLED, "yield supply on disabled reserve");
    let p1 = ctx.client.position();
    assert_eq!(p1.c_pool, p0.c_pool);
    assert_eq!(p1.d, p0.d);

    let delivered = ctx.progress(p0.c / 2);
    assert!(
        delivered > 0,
        "yield-pool withdraw allowed on disabled reserve"
    );
}

// ---------------------------------------------------------------------------
// Pause and realisation gates
// ---------------------------------------------------------------------------

#[test]
fn pause_blocks_supply_allows_unwind_and_deleverage() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.supply(10_0000000);
    let p0 = ctx.client.position();

    fx.env.mock_all_auths(); // set_paused is admin-gated
    ctx.client.set_paused(&ctx.admin, &true);
    assert!(ctx.client.paused());

    let res = ctx
        .client
        .try_supply(&ctx.vault, &ctx.fx.collateral_token, &1_0000000);
    assert!(
        matches!(res, Err(Ok(CarryError::Paused))),
        "supply blocked while paused"
    );
    assert_eq!(ctx.client.position().c_pool, p0.c_pool);

    let delivered = ctx.progress(p0.c / 2);
    assert!(delivered > 0, "unwind allowed while paused");

    ctx.deleverage();
    assert_eq!(
        ctx.client.position().d,
        0,
        "deleverage allowed while paused"
    );
    assert!(ctx.client.paused(), "pause flag untouched");
}

#[test]
fn realisation_pending_blocks_supply_and_total_assets_allows_unwind_and_deleverage() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.supply(10_0000000);
    let p0 = ctx.client.position();
    ctx.mint_idle_b(100_0000000);

    ctx.client.sweep_surplus(&ctx.admin);
    assert!(ctx.client.realisation_pending());

    let res = ctx
        .client
        .try_supply(&ctx.vault, &ctx.fx.collateral_token, &1_0000000);
    assert!(
        matches!(res, Err(Ok(CarryError::RealisationPending))),
        "supply blocked while pending"
    );
    let res = ctx.client.try_total_assets(&ctx.fx.collateral_token);
    assert!(
        matches!(res, Err(Ok(CarryError::RealisationPending))),
        "NAV blocked while pending"
    );
    assert_eq!(
        ctx.client.position().c_pool,
        p0.c_pool,
        "position unchanged"
    );

    // progress_withdrawal and deleverage keep working while pending.
    let delivered = ctx.progress(p0.c / 2);
    assert!(delivered > 0);
    ctx.deleverage();
    assert_eq!(ctx.client.position().d, 0);
    assert!(ctx.client.realisation_pending(), "flag still pending");
}

#[test]
fn sweep_surplus_zero_payout_rejected() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.supply(10_0000000);
    // Y == D exactly (buffer >= D keeps the whole leg): payout is 0.
    fx.env.mock_all_auths(); // admin auth for the sweep call
    let res = ctx.client.try_sweep_surplus(&ctx.admin);
    assert!(
        matches!(res, Err(Ok(CarryError::NothingToSweep))),
        "zero-payout sweep rejected"
    );
    assert!(!ctx.client.realisation_pending());
    assert_eq!(
        TokenClient::new(&fx.env, &ctx.fx.borrow_token).balance(&ctx.client.address),
        0,
        "recipient (whale) untouched in token terms"
    );
}

#[test]
fn idle_only_release_bypasses_pool_and_oracle() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.supply(10_0000000);
    let p0 = ctx.client.position();

    // Idle c only covers the pro-rata release; oracle goes stale; the pool
    // batch is Repay-only (idle-funded, x_pool == 0 -> no WithdrawCollateral,
    // no health check) so the unwind passes without any oracle read.
    let x_target = p0.c / 3;
    ctx.mint_collateral(x_target);
    fx.env
        .ledger()
        .set_timestamp(fx.env.ledger().timestamp() + 1_000);
    let oracle_calls_before = ctx.fx.oracle.call_count();

    let delivered = ctx.progress(x_target);
    assert!(delivered > 0 && delivered <= x_target);
    assert_eq!(
        ctx.fx.oracle.call_count(),
        oracle_calls_before,
        "adapter read no oracle price"
    );
    let p1 = ctx.client.position();
    assert!(p1.c_pool <= p0.c_pool, "collateral at least held");
    assert!(p1.d <= p0.d, "debt never grows");
}

// ---------------------------------------------------------------------------
// Cost estimate
// ---------------------------------------------------------------------------

#[test]
fn supply_and_progress_withdrawal_within_mainnet_limits() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    // Native build (not optimized wasm): VM instantiation and host-side
    // wasm execution of the pool are still metered; only the adapter's own
    // code runs as native Rust, so instruction counts understate the wasm
    // case by roughly a small constant factor.
    ctx.supply(10_0000000);
    let supply_res = fx.env.cost_estimate().resources();

    let p0 = ctx.client.position();
    let delivered = ctx.progress(p0.c + 1);
    assert_eq!(delivered, p0.c);
    let unwind_res = fx.env.cost_estimate().resources();

    // The network-level per-invocation limits (soroban-env-host's
    // InvocationResourceLimits::mainnet defaults; the type is not exported
    // through the SDK's public API).
    const MAINNET_CPU_INSNS: i64 = 100_000_000;
    const MAINNET_MEM_BYTES: i64 = 40_000_000;
    for (label, r) in [("supply", &supply_res), ("unwind", &unwind_res)] {
        assert!(
            r.instructions < MAINNET_CPU_INSNS,
            "{label}: {} instructions over mainnet limit {}",
            r.instructions,
            MAINNET_CPU_INSNS
        );
        assert!(
            r.mem_bytes < MAINNET_MEM_BYTES,
            "{label}: {} bytes over mainnet limit {}",
            r.mem_bytes,
            MAINNET_MEM_BYTES
        );
    }
}
