//! Thin helpers around the Blend pool client.
//!
//! All pool interaction goes through [`PoolOps`] so request-type constants,
//! auth entries and sparse-position handling live in one place. `Request`
//! request types are numeric (verified against the fork at 54afdae):
//! `Supply = 0`, `Withdraw = 1`, `SupplyCollateral = 2`,
//! `WithdrawCollateral = 3`, `Borrow = 4`, `Repay = 5`,
//! `DeleteLiquidationAuction = 9`.

use soroban_sdk::{
    auth::{ContractContext, InvokerContractAuthEntry, SubContractInvocation},
    vec, Address, Env, IntoVal, Symbol, Vec,
};

// The vendored fork artifacts. The clients are generated from the *fork* wasm,
// so reserve/pool/position types on the wire are exactly the fork's.
#[allow(clippy::too_many_arguments)] // generated pool client ABI
mod fork_pool {
    soroban_sdk::contractimport!(file = "tests/support/artifacts/pool.wasm");
}
#[allow(clippy::too_many_arguments)] // generated factory client ABI
mod fork_factory {
    soroban_sdk::contractimport!(file = "tests/support/artifacts/pool_factory.wasm");
}

// Factory client/meta and both wasms are host-harness only; the contract
// body itself only needs the pool wire types below.
#[cfg(any(test, feature = "testutils"))]
pub use fork_factory::{Client as ForkFactoryClient, PoolInitMeta, WASM as FACTORY_WASM};
pub use fork_pool::{Client as PoolClient, PoolConfig, Positions, Request, Reserve};
#[cfg(any(test, feature = "testutils"))]
pub use fork_pool::{ReserveConfig, WASM as POOL_WASM};

pub const REQUEST_SUPPLY: u32 = 0;
pub const REQUEST_WITHDRAW: u32 = 1;
pub const REQUEST_SUPPLY_COLLATERAL: u32 = 2;
pub const REQUEST_WITHDRAW_COLLATERAL: u32 = 3;
pub const REQUEST_BORROW: u32 = 4;
pub const REQUEST_REPAY: u32 = 5;
pub const REQUEST_DELETE_LIQ_AUCTION: u32 = 9;

/// The liquidation-auction type id (user liquidation auction).
pub const AUCTION_TYPE_LIQ: u32 = 0;

pub trait PoolOps {
    fn pool_config(&self, pool: &Address) -> PoolConfig;
    fn reserve(&self, pool: &Address, asset: &Address) -> Reserve;
    fn positions(&self, pool: &Address, who: &Address) -> Positions;
    fn balance(&self, token: &Address, who: &Address) -> i128;
    /// Whether `user` has a live liquidation auction on `pool`. (Deletion is
    /// done by pushing a `REQUEST_DELETE_LIQ_AUCTION` request into the same
    /// `submit` batch; the fork pool does not export a standalone delete.)
    fn has_liquidation_auction(&self, pool: &Address, user: &Address) -> bool;
    /// Submit `requests` with `adapter` as from/spender/to, after authorizing
    /// the token transfer `token` `from` -> `to` of `amount` (if `token` is
    /// `Some`). Auth is granted via `authorize_as_current_contract`, so the
    /// pool-side token pulls are exercised for real.
    fn submit_auth(
        &self,
        pool: &Address,
        adapter: &Address,
        requests: &Vec<Request>,
        token: Option<(&Address, &Address, i128)>,
    );
}

impl PoolOps for Env {
    fn pool_config(&self, pool: &Address) -> PoolConfig {
        PoolClient::new(self, pool).get_config()
    }

    fn reserve(&self, pool: &Address, asset: &Address) -> Reserve {
        PoolClient::new(self, pool).get_reserve(asset)
    }

    fn positions(&self, pool: &Address, who: &Address) -> Positions {
        PoolClient::new(self, pool).get_positions(who)
    }

    fn balance(&self, token: &Address, who: &Address) -> i128 {
        soroban_sdk::token::Client::new(self, token).balance(who)
    }

    fn has_liquidation_auction(&self, pool: &Address, user: &Address) -> bool {
        // `get_auction` panics when the auction is absent; the `try_` form
        // captures that as an error without reverting.
        PoolClient::new(self, pool)
            .try_get_auction(&AUCTION_TYPE_LIQ, user)
            .is_ok()
    }

    fn submit_auth(
        &self,
        pool: &Address,
        adapter: &Address,
        requests: &Vec<Request>,
        token: Option<(&Address, &Address, i128)>,
    ) {
        if let Some((token_addr, from, amount)) = token {
            self.authorize_as_current_contract(vec![
                self,
                InvokerContractAuthEntry::Contract(SubContractInvocation {
                    context: ContractContext {
                        contract: token_addr.clone(),
                        fn_name: Symbol::new(self, "transfer"),
                        args: (from.clone(), pool.clone(), amount).into_val(self),
                    },
                    sub_invocations: Vec::new(self),
                }),
            ]);
        }
        PoolClient::new(self, pool).submit(adapter, adapter, adapter, requests);
    }
}

/// Total underlying supply of a reserve (`to_asset_from_b_token`, floor).
pub fn reserve_supply(reserve: &Reserve) -> i128 {
    crate::math::b_token_underlying(reserve.data.b_supply, reserve.data.b_rate)
        .expect("b_supply non-negative, b_rate fits")
}

/// Total underlying liabilities of a reserve (`to_asset_from_d_token`, ceil).
pub fn reserve_liabilities(reserve: &Reserve) -> i128 {
    crate::math::debt_underlying(reserve.data.d_supply, reserve.data.d_rate)
        .expect("d_supply non-negative, d_rate fits")
}
