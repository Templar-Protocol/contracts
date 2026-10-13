//! Auth-tree tests (carry plan step 6).
//!
//! Every state-changing entrypoint is exercised with `env.mock_auths`
//! authorizing ONLY the root caller (vault or admin), so the adapter's
//! `authorize_as_current_contract` sub-invocation entries (collateral supply
//! into the pool, borrow-asset transfers into the yield pool, repay into the
//! pool) are exercised for real against the pinned fork pool. Wrong-caller
//! rejection is asserted on every state-changing entrypoint, and a
//! strip/restore battery shows that withholding the root auth or the
//! contract's own sub-auth entry makes the batch fail.

use soroban_sdk::{
    auth::{ContractContext, InvokerContractAuthEntry, SubContractInvocation},
    testutils::{Address as _, MockAuth, MockAuthInvoke},
    token::Client as TokenClient,
    vec, Address, BytesN, Env, IntoVal, Vec,
};
use templar_soroban_carry_adapter::contract::{
    CarryAdapterContract, CarryAdapterContractClient, CarryError, Params,
};
use templar_soroban_carry_adapter::testutils::{CarryFixture, PoolClient, Request, RequestType};

/// Authorizes `address` (and nothing else) for one invocation of
/// `fn_name(args...)` on the carry adapter.
macro_rules! auth {
    ($env:expr, $address:expr, $id:expr, $fn_name:literal, ( $($arg:expr),* $(,)? )) => {{
        let __env: &Env = $env;
        let args: Vec<soroban_sdk::Val> = ($($arg,)*).into_val(__env);
        MockAuth {
            address: $address,
            invoke: &MockAuthInvoke {
                contract: $id,
                fn_name: $fn_name,
                args,
                sub_invokes: &[],
            },
        }
    }};
}

struct Ctx<'a> {
    fx: &'a CarryFixture<'a>,
    client: CarryAdapterContractClient<'a>,
    admin: Address,
    vault: Address,
    stranger: Address,
    /// Immutable `surplus_recipient` (distinct from vault/admin).
    recipient: Address,
}

impl<'a> Ctx<'a> {
    fn new(fx: &'a CarryFixture<'a>) -> Self {
        let env = &fx.env;
        let admin = fx.admin.clone();
        let vault = fx.whale.clone();
        let stranger = Address::generate(env);
        let recipient = Address::generate(env);
        let id = env.register(
            CarryAdapterContract,
            (
                admin.clone(),
                vault.clone(),
                fx.pool.address.clone(),
                fx.yield_pool.address.clone(),
                fx.collateral_token.clone(),
                fx.borrow_token.clone(),
                recipient.clone(),
                8_000u32, // borrow_utilization_bps
                10u32,    // surplus_buffer_bps
                500u32,   // surplus_haircut_bps
                50u32,    // oracle_free_band_bps
                86_400u64,
                0i128, // min_residual_collateral
            ),
        );
        Self {
            fx,
            client: CarryAdapterContractClient::new(env, &id),
            admin,
            vault,
            stranger,
            recipient,
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
}

// ---------------------------------------------------------------------------
// Root-caller-only batches (authorize_as_current_contract exercised for real)
// ---------------------------------------------------------------------------

#[test]
fn supply_batch_root_vault_auth_only() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.vault,
        &ctx.client.address,
        "supply",
        (&ctx.vault, &ctx.fx.collateral_token, 10_0000000i128)
    )]);
    ctx.client
        .supply(&ctx.vault, &ctx.fx.collateral_token, &10_0000000);

    let pos = ctx.client.position();
    assert_eq!(pos.c_pool, 10_0000000, "collateral supplied to the pool");
    assert!(pos.d > 0, "borrow leg opened");
    assert!(pos.y_pool > 0, "yield leg deployed");
    assert_eq!(pos.y_pool, pos.d, "full borrow deployed into yield leg");
    // The pool pulled the collateral via the adapter's own sub-auth.
    let tc = TokenClient::new(&fx.env, &ctx.fx.collateral_token);
    assert_eq!(tc.balance(&ctx.client.address), 0);
}

#[test]
fn unwind_batch_root_vault_auth_only_idle_funded() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.client
        .supply(&ctx.vault, &ctx.fx.collateral_token, &10_0000000);
    let p0 = ctx.client.position();
    assert!(p0.d > 0);
    // Idle b covers the whole pro-rata repayment -> the pool batch is
    // authorized for exactly that pull.
    ctx.mint_idle_b(p0.d);

    let vault_before = TokenClient::new(&fx.env, &ctx.fx.collateral_token).balance(&ctx.vault);
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.vault,
        &ctx.client.address,
        "progress_withdrawal",
        (&ctx.vault, &ctx.fx.collateral_token, (p0.c + 1))
    )]);
    let delivered =
        ctx.client
            .progress_withdrawal(&ctx.vault, &ctx.fx.collateral_token, &(p0.c + 1));
    assert_eq!(delivered, p0.c, "full unwind delivers all collateral");
    assert_eq!(
        TokenClient::new(&fx.env, &ctx.fx.collateral_token).balance(&ctx.vault),
        vault_before + p0.c
    );
    let pos = ctx.client.position();
    assert_eq!(pos.c_pool, 0, "collateral released");
    assert_eq!(pos.d, 0, "debt repaid");
    assert_eq!(
        pos.y_pool, p0.y_pool,
        "idle b funds the repay; yield leg untouched"
    );
}

#[test]
fn unwind_batch_root_vault_auth_only_yield_funded() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.client
        .supply(&ctx.vault, &ctx.fx.collateral_token, &10_0000000);
    let p0 = ctx.client.position();
    // No idle b: the repayment must be funded from the yield leg (a
    // yield-pool Withdraw sub-call plus the Repay pull on `pool`).
    assert_eq!(p0.idle_b, 0);

    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.vault,
        &ctx.client.address,
        "progress_withdrawal",
        (&ctx.vault, &ctx.fx.collateral_token, (p0.c + 1))
    )]);
    let delivered =
        ctx.client
            .progress_withdrawal(&ctx.vault, &ctx.fx.collateral_token, &(p0.c + 1));
    assert_eq!(delivered, p0.c);
    let pos = ctx.client.position();
    assert_eq!(pos.c_pool, 0);
    assert_eq!(pos.d, 0);
    assert_eq!(pos.y_pool, 0);
}

#[test]
fn deleverage_batch_root_admin_auth_only() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.client
        .supply(&ctx.vault, &ctx.fx.collateral_token, &10_0000000);
    let p0 = ctx.client.position();
    assert!(p0.d > 0);
    // No idle b: deleverage funds the repay from the yield leg.
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.admin,
        &ctx.client.address,
        "deleverage",
        (&ctx.admin,)
    )]);
    ctx.client.deleverage(&ctx.admin);
    let pos = ctx.client.position();
    assert_eq!(pos.d, 0, "debt repaid from the yield leg");
    assert_eq!(pos.c_pool, p0.c_pool, "collateral untouched");
    assert_eq!(pos.y_pool, 0, "yield leg withdrawn to fund the repay");
}

#[test]
fn sweep_surplus_root_admin_auth_only() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.client
        .supply(&ctx.vault, &ctx.fx.collateral_token, &10_0000000);
    ctx.mint_idle_b(100_0000000);
    let recipient_before = TokenClient::new(&fx.env, &ctx.fx.borrow_token).balance(&ctx.recipient);
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.admin,
        &ctx.client.address,
        "sweep_surplus",
        (&ctx.admin,)
    )]);
    ctx.client.sweep_surplus(&ctx.admin);
    assert!(ctx.client.realisation_pending());
    let recipient_after = TokenClient::new(&fx.env, &ctx.fx.borrow_token).balance(&ctx.recipient);
    assert!(recipient_after > recipient_before, "surplus paid out");
}

// ---------------------------------------------------------------------------
// Strip / restore: withholding an auth entry fails the batch
// ---------------------------------------------------------------------------

#[test]
fn strip_root_auth_each_batch_fails_then_restores() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);

    // -- supply batch, root auth withheld -> auth failure, nothing booked.
    // `mock_auths` registers the expected tree; authorizing only the
    // stranger (not the vault) strips the root caller.
    ctx.mint_collateral(10_0000000);
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.stranger,
        &ctx.client.address,
        "supply",
        (&ctx.stranger, &ctx.fx.collateral_token, 10_0000000i128)
    )]);
    let res = ctx
        .client
        .try_supply(&ctx.stranger, &ctx.fx.collateral_token, &10_0000000);
    assert!(res.is_err(), "supply without vault auth must fail");
    assert_eq!(ctx.client.position().c_pool, 0, "nothing supplied");
    // The adapter's idle collateral was untouched by the reverted call.
    assert_eq!(
        TokenClient::new(&fx.env, &ctx.fx.collateral_token).balance(&ctx.client.address),
        10_0000000
    );
    // -- restored: same call succeeds.
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.vault,
        &ctx.client.address,
        "supply",
        (&ctx.vault, &ctx.fx.collateral_token, 10_0000000i128)
    )]);
    ctx.client
        .supply(&ctx.vault, &ctx.fx.collateral_token, &10_0000000);
    assert_eq!(ctx.client.position().c_pool, 10_0000000);

    // -- unwind batch, root auth withheld.
    let p0 = ctx.client.position();
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.stranger,
        &ctx.client.address,
        "progress_withdrawal",
        (&ctx.stranger, &ctx.fx.collateral_token, (p0.c + 1))
    )]);
    let res =
        ctx.client
            .try_progress_withdrawal(&ctx.stranger, &ctx.fx.collateral_token, &(p0.c + 1));
    assert!(res.is_err(), "unwind without vault auth must fail");
    assert_eq!(ctx.client.position().c_pool, p0.c_pool, "position intact");
    // -- restored.
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.vault,
        &ctx.client.address,
        "progress_withdrawal",
        (&ctx.vault, &ctx.fx.collateral_token, (p0.c + 1))
    )]);
    let delivered =
        ctx.client
            .progress_withdrawal(&ctx.vault, &ctx.fx.collateral_token, &(p0.c + 1));
    assert_eq!(delivered, p0.c);
    assert_eq!(ctx.client.position().c_pool, 0);

    // -- deleverage batch, root auth withheld.
    ctx.mint_collateral(10_0000000);
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.vault,
        &ctx.client.address,
        "supply",
        (&ctx.vault, &ctx.fx.collateral_token, 10_0000000i128)
    )]);
    ctx.client
        .supply(&ctx.vault, &ctx.fx.collateral_token, &10_0000000);
    assert!(ctx.client.position().d > 0);
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.stranger,
        &ctx.client.address,
        "deleverage",
        (&ctx.stranger,)
    )]);
    let res = ctx.client.try_deleverage(&ctx.stranger);
    assert!(res.is_err(), "deleverage without admin auth must fail");
    assert!(ctx.client.position().d > 0, "debt intact");
    // -- restored.
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.admin,
        &ctx.client.address,
        "deleverage",
        (&ctx.admin,)
    )]);
    ctx.client.deleverage(&ctx.admin);
    assert_eq!(ctx.client.position().d, 0);

    // -- sweep batch, root auth withheld.
    ctx.mint_idle_b(100_0000000);
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.stranger,
        &ctx.client.address,
        "sweep_surplus",
        (&ctx.stranger,)
    )]);
    let res = ctx.client.try_sweep_surplus(&ctx.stranger);
    assert!(res.is_err(), "sweep without admin auth must fail");
    assert!(!ctx.client.realisation_pending(), "flag untouched");
    assert_eq!(
        TokenClient::new(&fx.env, &ctx.fx.borrow_token).balance(&ctx.recipient),
        0,
        "recipient untouched"
    );
    // -- restored.
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.admin,
        &ctx.client.address,
        "sweep_surplus",
        (&ctx.admin,)
    )]);
    ctx.client.sweep_surplus(&ctx.admin);
    assert!(ctx.client.realisation_pending());
}

#[test]
fn strip_root_auth_entrypoint_args_must_match() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    // Vault is authorized for supply of 10_0000000 but supplies 5_0000000.
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.vault,
        &ctx.client.address,
        "supply",
        (&ctx.vault, &ctx.fx.collateral_token, 10_0000000i128)
    )]);
    let res = ctx
        .client
        .try_supply(&ctx.vault, &ctx.fx.collateral_token, &5_0000000);
    assert!(res.is_err(), "arg mismatch must fail the auth check");
    assert_eq!(ctx.client.position().c_pool, 0);
}

/// The pool token pull inside a batch depends on the adapter's own
/// `authorize_as_current_contract` sub-invocation entry: the same
/// pool `submit` submitted without it must fail the pool-side auth check.
#[test]
fn strip_contract_sub_auth_pool_pull_fails() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    let adapter = ctx.client.address.clone();
    let pool = ctx.fx.pool.address.clone();
    let collateral = ctx.fx.collateral_token.clone();
    ctx.mint_collateral(10_0000000);

    // (a) sub-auth withheld: submit fails the pool-side auth check.
    fx.env.mock_auths(&[]);
    fx.env.as_contract(&adapter, || {
        let requests = vec![
            &fx.env,
            Request {
                request_type: RequestType::SupplyCollateral as u32,
                address: collateral.clone(),
                amount: 10_0000000,
            },
        ];
        let client = PoolClient::new(&fx.env, &pool);
        let res = client.try_submit(&adapter, &adapter, &adapter, &requests);
        assert!(res.is_err(), "pool submit without sub-auth must fail");
    });
    assert_eq!(ctx.client.position().c_pool, 0, "nothing supplied");

    // (b) sub-auth amount tampered: fails the same way.
    fx.env.as_contract(&adapter, || {
        fx.env.authorize_as_current_contract(vec![
            &fx.env,
            InvokerContractAuthEntry::Contract(SubContractInvocation {
                context: ContractContext {
                    contract: collateral.clone(),
                    fn_name: soroban_sdk::symbol_short!("transfer"),
                    args: (adapter.clone(), pool.clone(), 10_0000001i128).into_val(&fx.env),
                },
                sub_invocations: Vec::new(&fx.env),
            }),
        ]);
        let requests = vec![
            &fx.env,
            Request {
                request_type: RequestType::SupplyCollateral as u32,
                address: collateral.clone(),
                amount: 10_0000000,
            },
        ];
        let client = PoolClient::new(&fx.env, &pool);
        let res = client.try_submit(&adapter, &adapter, &adapter, &requests);
        assert!(res.is_err(), "tampered sub-auth amount must fail");
    });
    assert_eq!(ctx.client.position().c_pool, 0);

    // (c) restored: the supply batch succeeds.
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.vault,
        &ctx.client.address,
        "supply",
        (&ctx.vault, &ctx.fx.collateral_token, 10_0000000i128)
    )]);
    ctx.client
        .supply(&ctx.vault, &ctx.fx.collateral_token, &10_0000000);
    assert_eq!(ctx.client.position().c_pool, 10_0000000);
}

// ---------------------------------------------------------------------------
// Wrong-caller rejection on every state-changing entrypoint
// ---------------------------------------------------------------------------

macro_rules! assert_unauthorized {
    ($res:expr) => {{
        match $res {
            Err(Ok(e)) => {
                let e: soroban_sdk::Error = e.into();
                assert_eq!(
                    e,
                    soroban_sdk::Error::from_contract_error(CarryError::Unauthorized as u32),
                    "expected Unauthorized contract error"
                );
            }
            Err(Err(_)) => panic!("host-level failure, expected Unauthorized contract error"),
            Ok(_) => panic!("expected Unauthorized, call succeeded"),
        }
    }};
}

#[test]
fn supply_wrong_caller_rejected() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    // admin is authorized but is not the vault
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.admin,
        &ctx.client.address,
        "supply",
        (&ctx.admin, &ctx.fx.collateral_token, 10_0000000i128)
    )]);
    let res = ctx
        .client
        .try_supply(&ctx.admin, &ctx.fx.collateral_token, &10_0000000);
    assert_unauthorized!(res);
    assert_eq!(ctx.client.position().c_pool, 0, "state unchanged");

    // stranger authorized for the same call
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.stranger,
        &ctx.client.address,
        "supply",
        (&ctx.stranger, &ctx.fx.collateral_token, 10_0000000i128)
    )]);
    let res = ctx
        .client
        .try_supply(&ctx.stranger, &ctx.fx.collateral_token, &10_0000000);
    assert_unauthorized!(res);
    assert_eq!(ctx.client.position().c_pool, 0);

    // impersonation: caller claims to be the vault but only the stranger
    // can sign -> the vault's auth check fails
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.stranger,
        &ctx.client.address,
        "supply",
        (&ctx.vault, &ctx.fx.collateral_token, 10_0000000i128)
    )]);
    let res = ctx
        .client
        .try_supply(&ctx.vault, &ctx.fx.collateral_token, &10_0000000);
    assert!(res.is_err(), "impersonated caller must fail");
}

#[test]
fn progress_withdrawal_wrong_caller_rejected() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.client
        .supply(&ctx.vault, &ctx.fx.collateral_token, &10_0000000);
    let p0 = ctx.client.position();

    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.admin,
        &ctx.client.address,
        "progress_withdrawal",
        (&ctx.admin, &ctx.fx.collateral_token, (p0.c + 1))
    )]);
    let res = ctx
        .client
        .try_progress_withdrawal(&ctx.admin, &ctx.fx.collateral_token, &(p0.c + 1));
    assert_unauthorized!(res);
    assert_eq!(
        ctx.client.position().c_pool,
        p0.c_pool,
        "position unchanged"
    );

    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.stranger,
        &ctx.client.address,
        "progress_withdrawal",
        (&ctx.stranger, &ctx.fx.collateral_token, (p0.c + 1))
    )]);
    let res =
        ctx.client
            .try_progress_withdrawal(&ctx.stranger, &ctx.fx.collateral_token, &(p0.c + 1));
    assert_unauthorized!(res);
}

#[test]
fn set_paused_wrong_caller_rejected_admin_and_vault_accepted() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.stranger,
        &ctx.client.address,
        "set_paused",
        (&ctx.stranger, false)
    )]);
    let res = ctx.client.try_set_paused(&ctx.stranger, &false);
    assert_unauthorized!(res);
    assert!(!ctx.client.paused(), "pause flag unchanged");

    // Admin or vault may pause (blend-adapter policy).
    for caller in [&ctx.admin, &ctx.vault] {
        fx.env.mock_auths(&[auth!(
            &fx.env,
            caller,
            &ctx.client.address,
            "set_paused",
            (caller, true)
        )]);
        ctx.client.set_paused(caller, &true);
        assert!(ctx.client.paused());
        fx.env.mock_auths(&[auth!(
            &fx.env,
            caller,
            &ctx.client.address,
            "set_paused",
            (caller, false)
        )]);
        ctx.client.set_paused(caller, &false);
        assert!(!ctx.client.paused());
    }
}

#[test]
fn set_params_wrong_caller_rejected() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    let params = Params {
        borrow_utilization_bps: 8_000,
        surplus_buffer_bps: 10,
        surplus_haircut_bps: 500,
        oracle_free_band_bps: 50,
        oracle_max_age_secs: 86_400,
        min_residual_collateral: 0,
    };
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.stranger,
        &ctx.client.address,
        "set_params",
        (&ctx.stranger, &params)
    )]);
    let res = ctx.client.try_set_params(&ctx.stranger, &params);
    assert_unauthorized!(res);

    // admin accepts and an out-of-bounds param set is rejected
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.admin,
        &ctx.client.address,
        "set_params",
        (&ctx.admin, &params)
    )]);
    ctx.client.set_params(&ctx.admin, &params);
    let mut bad = params;
    bad.borrow_utilization_bps = 9_001; // > MAX_BORROW_UTILIZATION_BPS
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.admin,
        &ctx.client.address,
        "set_params",
        (&ctx.admin, &bad)
    )]);
    let res = ctx.client.try_set_params(&ctx.admin, &bad);
    assert!(matches!(res, Err(Ok(CarryError::InvalidParams))));
}

#[test]
fn deleverage_wrong_caller_rejected() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.client
        .supply(&ctx.vault, &ctx.fx.collateral_token, &10_0000000);
    let p0 = ctx.client.position();

    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.stranger,
        &ctx.client.address,
        "deleverage",
        (&ctx.stranger,)
    )]);
    let res = ctx.client.try_deleverage(&ctx.stranger);
    assert_unauthorized!(res);
    assert_eq!(ctx.client.position().d, p0.d, "debt unchanged");

    // the vault is also not an admin
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.vault,
        &ctx.client.address,
        "deleverage",
        (&ctx.vault,)
    )]);
    let res = ctx.client.try_deleverage(&ctx.vault);
    assert_unauthorized!(res);
}

#[test]
fn sweep_surplus_wrong_caller_rejected() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.client
        .supply(&ctx.vault, &ctx.fx.collateral_token, &10_0000000);
    ctx.mint_idle_b(100_0000000);

    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.stranger,
        &ctx.client.address,
        "sweep_surplus",
        (&ctx.stranger,)
    )]);
    let res = ctx.client.try_sweep_surplus(&ctx.stranger);
    assert_unauthorized!(res);
    assert!(!ctx.client.realisation_pending());
    assert_eq!(
        TokenClient::new(&fx.env, &ctx.fx.borrow_token).balance(&ctx.recipient),
        0
    );

    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.vault,
        &ctx.client.address,
        "sweep_surplus",
        (&ctx.vault,)
    )]);
    let res = ctx.client.try_sweep_surplus(&ctx.vault);
    assert_unauthorized!(res);
}

#[test]
fn clear_realisation_pending_wrong_caller_rejected() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    ctx.mint_collateral(10_0000000);
    ctx.client
        .supply(&ctx.vault, &ctx.fx.collateral_token, &10_0000000);
    ctx.mint_idle_b(100_0000000);
    ctx.client.sweep_surplus(&ctx.admin);
    assert!(ctx.client.realisation_pending());

    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.stranger,
        &ctx.client.address,
        "clear_realisation_pending",
        (&ctx.stranger,)
    )]);
    let res = ctx.client.try_clear_realisation_pending(&ctx.stranger);
    assert_unauthorized!(res);
    assert!(ctx.client.realisation_pending(), "flag unchanged");

    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.vault,
        &ctx.client.address,
        "clear_realisation_pending",
        (&ctx.vault,)
    )]);
    let res = ctx.client.try_clear_realisation_pending(&ctx.vault);
    assert_unauthorized!(res);
    assert!(ctx.client.realisation_pending());

    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.admin,
        &ctx.client.address,
        "clear_realisation_pending",
        (&ctx.admin,)
    )]);
    ctx.client.clear_realisation_pending(&ctx.admin);
    assert!(!ctx.client.realisation_pending());
}

#[test]
fn set_admin_accept_admin_wrong_caller_rejected() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    let new_admin = Address::generate(&fx.env);

    // stranger cannot even queue the change
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.stranger,
        &ctx.client.address,
        "set_admin",
        (&ctx.stranger, new_admin.clone())
    )]);
    let res = ctx.client.try_set_admin(&ctx.stranger, &new_admin);
    assert_unauthorized!(res);
    assert_eq!(ctx.client.admin(), ctx.admin);

    // queueing via the admin works
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.admin,
        &ctx.client.address,
        "set_admin",
        (&ctx.admin, new_admin.clone())
    )]);
    ctx.client.set_admin(&ctx.admin, &new_admin);

    // a stranger (authorized for accept_admin, but caller != pending) fails
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.stranger,
        &ctx.client.address,
        "accept_admin",
        (&ctx.stranger,)
    )]);
    let res = ctx.client.try_accept_admin(&ctx.stranger);
    assert!(res.is_err(), "stranger cannot accept admin");
    assert_eq!(ctx.client.admin(), ctx.admin);

    // the pending admin accepts; the OLD admin can no longer act
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &new_admin,
        &ctx.client.address,
        "accept_admin",
        (&new_admin,)
    )]);
    ctx.client.accept_admin(&new_admin);
    assert_eq!(ctx.client.admin(), new_admin);
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.admin,
        &ctx.client.address,
        "set_paused",
        (&ctx.admin, true)
    )]);
    let res = ctx.client.try_set_paused(&ctx.admin, &true);
    assert_unauthorized!(res);
}

#[test]
fn upgrade_wrong_caller_rejected() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    let fake_hash = BytesN::from_array(
        &fx.env,
        &[
            1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
            24, 25, 26, 27, 28, 29, 30, 31, 32,
        ],
    );
    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.vault,
        &ctx.client.address,
        "upgrade",
        (&fake_hash, &ctx.vault)
    )]);
    let res = ctx.client.try_upgrade(&fake_hash, &ctx.vault);
    assert!(res.is_err(), "vault cannot upgrade");

    fx.env.mock_auths(&[auth!(
        &fx.env,
        &ctx.stranger,
        &ctx.client.address,
        "upgrade",
        (&fake_hash, &ctx.stranger)
    )]);
    let res = ctx.client.try_upgrade(&fake_hash, &ctx.stranger);
    assert!(res.is_err(), "stranger cannot upgrade");
}

#[test]
fn views_and_total_assets_need_no_auth() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let ctx = Ctx::new(&fx);
    fx.env.mock_auths(&[]);
    assert!(!ctx.client.realisation_pending());
    assert!(!ctx.client.paused());
    let pos = ctx.client.position();
    assert_eq!(pos.c, 0);
    assert_eq!(ctx.client.ltv(), 0);
}
