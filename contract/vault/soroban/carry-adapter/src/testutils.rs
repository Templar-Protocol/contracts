//! Test-only fixtures for the carry-adapter.
//!
//! Compiled for host test builds (`cargo test`) and for dependents that enable
//! the `testutils` feature (e.g. `soroban/tests/carry_e2e.rs`). The harness
//! deploys the *sha256-pinned vendored fork* pool and pool-factory wasm
//! (see `tests/support/artifacts/README.md`) against an in-crate
//! [`MockBackstopContract`] that implements exactly the backstop surface the fork pool
//! calls (`pool_data`, `backstop_token`, `gulp_emissions`, `draw`, `donate`).
//! Two pools are brought to status 0 (admin active) through the pool's genuine
//! threshold/q4w checks, and a settable SEP-40 mock oracle counts price reads.
//!
//! Third-party liquidity and borrow helpers (`third_party_supply`,
//! `third_party_borrow`, `third_party_repay`) exist on BOTH pools so tests can
//! force utilization/accrual conditions the adapter itself must handle.

use soroban_sdk::{
    contract, contractimpl, symbol_short,
    testutils::{Address as _, BytesN as _, Ledger as _},
    vec, Address, Bytes, BytesN, Env, IntoVal, String, Symbol, TryIntoVal, Val, Vec,
};

// The fork pool client, its config/position wire types and the pool-factory
// client live in `pool_ops` (production code); the harness uses them too.
pub use crate::pool_ops::{
    ForkFactoryClient, PoolClient, PoolConfig, PoolInitMeta, Positions, Request, Reserve,
    ReserveConfig, FACTORY_WASM, POOL_WASM,
};

// Emitter fixture is stock (blend-contract-sdk); the pool stack is the fork.
use blend_contract_sdk::emitter as emitter_fixture;

/// Blend v2 `submit` request discriminants, per the fork's
/// `pool/src/pool/actions.rs` (the vendored pool wasm is built from that
/// source). The pool spec encodes `Request.request_type` as a plain u32, so
/// contractimport does not export the enum; values mirrored here.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum RequestType {
    Supply = 0,
    Withdraw = 1,
    SupplyCollateral = 2,
    WithdrawCollateral = 3,
    Borrow = 4,
    Repay = 5,
}

/// SHA-256 pin for the vendored fork pool wasm. See artifacts/README.md.
pub const POOL_WASM_SHA256: [u8; 32] = [
    0x2d, 0xb0, 0x4d, 0x55, 0xfa, 0x91, 0x24, 0x6f, 0x26, 0x3c, 0x7c, 0xd9, 0x8b, 0xa9, 0xb4, 0x36,
    0xa9, 0x7e, 0x5f, 0x32, 0x21, 0x8d, 0x67, 0xa8, 0x6e, 0xa7, 0xbe, 0xca, 0x05, 0x79, 0x39, 0xfa,
];
/// SHA-256 pin for the vendored fork pool-factory wasm.
pub const POOL_FACTORY_WASM_SHA256: [u8; 32] = [
    0x80, 0xab, 0xaf, 0x3c, 0x68, 0xfd, 0x15, 0xad, 0xad, 0xe9, 0xf8, 0x21, 0x65, 0xba, 0xa9, 0xb5,
    0x0d, 0x85, 0x80, 0x0e, 0x49, 0x7c, 0x99, 0xb4, 0x58, 0x09, 0xe1, 0xfb, 0xd7, 0x39, 0x7d, 0xe2,
];

/// Asserts the vendored fork artifacts hash to their pinned values.
///
/// Fails loudly if anyone swaps the wasm bytes (or the pins) out of sync.
/// `harness_smoke.rs` calls this first.
pub fn verify_artifact_pins(env: &Env) {
    let digest = |bytes: &[u8], pin: &[u8; 32], name: &str| {
        let mut b = Bytes::new(env);
        b.extend_from_slice(bytes);
        let got = env.crypto().sha256(&b);
        assert_eq!(got.to_bytes().to_array(), *pin, "wasm pin mismatch: {name}");
    };
    digest(POOL_WASM, &POOL_WASM_SHA256, "pool");
    digest(FACTORY_WASM, &POOL_FACTORY_WASM_SHA256, "pool_factory");
}

/********** Mock oracle (SEP-40, settable, call counter) **********/

/// Price feed shape stored per asset.
#[soroban_sdk::contracttype]
#[derive(Clone)]
pub struct MockOraclePrice {
    pub price: i128,
    pub timestamp: u64,
}

/// SEP-40 asset reference (ABI-compatible with `sep_40_oracle::Asset` and
/// the adapter's local `oracle_client::Asset`: `Stellar` is variant 0).
#[soroban_sdk::contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MockOracleAsset {
    Stellar(Address),
    Other(Symbol),
}

/// SEP-40 mock oracle with a per-call counter.
///
/// The pool reads `get_price`; the adapter reads `decimals`/`lastprice`. The
/// counter tracks every price read so NAV tests can assert the oracle-free
/// band (zero calls inside the band).
#[contract]
pub struct MockOracleContract;

/// Error emitted when the stored price data is invalid (absent asset, stale or
/// future timestamp) — the validity rules the pool relies on.
#[soroban_sdk::contracterror]
#[repr(u32)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum MockOracleError {
    InvalidPrice = 100,
    BadRequest = 101,
}

#[contractimpl]
impl MockOracleContract {
    /// Stores the base asset, the asset list, the shared price decimals and
    /// the staleness threshold in one shot (mirrors `set_data` on the
    /// protocol mock so fixtures read naturally).
    pub fn set_data(
        e: Env,
        base: Symbol,
        assets_by_address: Vec<Address>,
        decimals: u32,
        threshold: u32,
    ) {
        e.storage().instance().set(&symbol_short!("base"), &base);
        e.storage()
            .instance()
            .set(&symbol_short!("assets"), &assets_by_address);
        e.storage()
            .instance()
            .set(&symbol_short!("decimals"), &decimals);
        e.storage()
            .instance()
            .set(&symbol_short!("thresh"), &threshold);
    }

    /// Sets the stable price for every asset in `assets_by_address`, stamped
    /// with the current ledger timestamp.
    pub fn set_price(e: Env, assets_by_address: Vec<Address>, price: i128) {
        let ts = e.ledger().timestamp();
        for asset in assets_by_address.iter() {
            e.storage().persistent().set(
                &asset,
                &MockOraclePrice {
                    price,
                    timestamp: ts,
                },
            );
        }
    }

    /// SEP-40 `lastprice(Asset)`: like `get_price` but returns `None`
    /// instead of erroring for absent / stale / future prices (stale-or-
    /// future means the price is outside the freshness threshold in either
    /// direction). Non-`Stellar` assets are unpriced.
    pub fn lastprice(e: Env, asset: MockOracleAsset) -> Option<MockOraclePrice> {
        let MockOracleAsset::Stellar(asset) = asset else {
            return None;
        };
        let count: u32 = e
            .storage()
            .persistent()
            .get(&symbol_short!("calls"))
            .unwrap_or(0);
        e.storage()
            .persistent()
            .set(&symbol_short!("calls"), &(count + 1));
        let price: MockOraclePrice = e.storage().persistent().get(&asset)?;
        let threshold: u32 = e
            .storage()
            .instance()
            .get(&symbol_short!("thresh"))
            .unwrap_or(0);
        let now = e.ledger().timestamp();
        let age = if price.timestamp > now {
            price.timestamp - now
        } else {
            now - price.timestamp
        };
        if age > threshold as u64 {
            return None;
        }
        Some(price)
    }

    /// SEP-40 `decimals`.
    pub fn decimals(e: Env) -> u32 {
        e.storage()
            .instance()
            .get(&symbol_short!("decimals"))
            .unwrap_or(0)
    }

    /// Sets the price for a single asset stamped with `timestamp` (caller
    /// controls freshness: stale / future prices for negative-path tests).
    pub fn set_price_at(e: Env, asset: Address, price: i128, timestamp: u64) {
        e.storage()
            .persistent()
            .set(&asset, &MockOraclePrice { price, timestamp });
    }

    /// SEP-40 `get_price`: bumps the counter, then enforces the same validity
    /// rules the pool relies on (known asset, non-future fresh timestamp).
    pub fn get_price(e: Env, asset: Address) -> Result<MockOraclePrice, MockOracleError> {
        let count: u32 = e
            .storage()
            .persistent()
            .get(&symbol_short!("calls"))
            .unwrap_or(0);
        e.storage()
            .persistent()
            .set(&symbol_short!("calls"), &(count + 1));

        let price: MockOraclePrice = e
            .storage()
            .persistent()
            .get(&asset)
            .ok_or(MockOracleError::InvalidPrice)?;
        let threshold: u32 = e
            .storage()
            .instance()
            .get(&symbol_short!("thresh"))
            .unwrap_or(0);
        let now = e.ledger().timestamp();
        if price.timestamp > now || price.timestamp + (threshold as u64) < now {
            return Err(MockOracleError::InvalidPrice);
        }
        Ok(price)
    }

    /// Number of `get_price` calls served since deployment.
    pub fn call_count(e: Env) -> u32 {
        e.storage()
            .persistent()
            .get(&symbol_short!("calls"))
            .unwrap_or(0)
    }
}

/// Thin dynamic client for [`MockOracleContract`].
///
/// Deliberately dynamic (no generated client): keeps the mock out of the
/// generated-client surface and lets this file stay the single harness entry
/// point for later test agents.
pub struct MockOracleClient<'a> {
    env: &'a Env,
    address: Address,
}

impl<'a> MockOracleClient<'a> {
    pub fn new(env: &'a Env, address: &Address) -> Self {
        Self {
            env,
            address: address.clone(),
        }
    }

    pub fn address(&self) -> &Address {
        &self.address
    }

    /// Calls `set_data` on the mock oracle.
    pub fn set_data(
        &self,
        base: &Symbol,
        assets_by_address: &Vec<Address>,
        decimals: &u32,
        threshold: &u32,
    ) {
        let args = Vec::from_array(
            self.env,
            [
                base.clone().into_val(self.env),
                assets_by_address.clone().into_val(self.env),
                decimals.into_val(self.env),
                threshold.into_val(self.env),
            ],
        );
        let _: Val =
            self.env
                .invoke_contract(&self.address, &Symbol::new(self.env, "set_data"), args);
    }

    /// Calls `set_price` on the mock oracle.
    pub fn set_price(&self, assets_by_address: &Vec<Address>, price: &i128) {
        let args = Vec::from_array(
            self.env,
            [
                assets_by_address.clone().into_val(self.env),
                price.into_val(self.env),
            ],
        );
        let _: Val =
            self.env
                .invoke_contract(&self.address, &Symbol::new(self.env, "set_price"), args);
    }

    /// Number of price reads (`get_price` / `lastprice`) served since
    /// deployment.
    pub fn call_count(&self) -> u32 {
        let v: Val = self.env.invoke_contract(
            &self.address,
            &Symbol::new(self.env, "call_count"),
            Vec::new(self.env),
        );
        v.try_into_val(self.env).unwrap()
    }
}

/********** Fork reserve defaults **********/

/// Default reserve config mirrored from the fork's own test defaults
/// (`pool/src/testutils.rs::default_reserve_meta`) so harness pools price the
/// way the fork's own tests expect.
pub fn default_reserve_metadata() -> ReserveConfig {
    ReserveConfig {
        decimals: 7,
        c_factor: 7_500_000,
        l_factor: 7_500_000,
        util: 7_500_000,
        max_util: 9_500_000,
        r_base: 100_000,
        r_one: 500_000,
        r_two: 5_000_000,
        r_three: 1_5000000,
        reactivity: 20,
        index: 0,
        supply_cap: i128::MAX,
        enabled: true,
    }
}

/********** Mock backstop **********/

/// Mirror of the fork's `backstop::PoolBackstopData` (field-for-field; the
/// pool imports this exact struct from the fork backstop wasm spec).
#[soroban_sdk::contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PoolBackstopData {
    pub tokens: i128,
    pub shares: i128,
    pub q4w_pct: i128,
    pub blnd: i128,
    pub usdc: i128,
    pub token_spot_price: i128,
}

/// Fork `BackstopError` subset (`backstop/src/errors.rs`).
#[soroban_sdk::contracterror]
#[repr(u32)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum MockBackstopError {
    BadRequest = 1000,
    NotPool = 1004,
}

#[soroban_sdk::contracttype]
#[derive(Clone)]
enum MockBackstopKey {
    Token,
    Data(Address),
}

/// Minimal backstop for the fork pool: only the functions the pool invokes
/// (`pool/src/{pool/status,pool/bad_debt,auctions/*,emissions/manager}.rs`).
/// Pool backstop data is settable so tests can drive the pool's genuine
/// threshold / q4w status logic; `draw`/`donate` move the real backstop token.
/// `gulp_emissions` panics `BadRequest` exactly like the fork backstop.
#[contract]
pub struct MockBackstopContract;

#[contractimpl]
impl MockBackstopContract {
    pub fn __constructor(e: Env, backstop_token: Address) {
        e.storage()
            .instance()
            .set(&MockBackstopKey::Token, &backstop_token);
    }

    /// Test control: sets the data `pool_data(pool)` reports.
    pub fn set_pool_data(e: Env, pool: Address, data: PoolBackstopData) {
        e.storage()
            .persistent()
            .set(&MockBackstopKey::Data(pool), &data);
    }

    pub fn pool_data(e: Env, pool: Address) -> PoolBackstopData {
        e.storage()
            .persistent()
            .get(&MockBackstopKey::Data(pool))
            .unwrap_or(PoolBackstopData {
                tokens: 0,
                shares: 0,
                q4w_pct: 0,
                blnd: 0,
                usdc: 0,
                token_spot_price: 0,
            })
    }

    pub fn backstop_token(e: Env) -> Address {
        e.storage().instance().get(&MockBackstopKey::Token).unwrap()
    }

    pub fn gulp_emissions(e: Env, _pool: Address) -> i128 {
        soroban_sdk::panic_with_error!(&e, MockBackstopError::BadRequest);
    }

    pub fn draw(e: Env, pool_address: Address, amount: i128, to: Address) {
        pool_address.require_auth();
        let mut data = Self::pool_data(e.clone(), pool_address.clone());
        if amount < 0 || amount > data.tokens {
            soroban_sdk::panic_with_error!(&e, MockBackstopError::BadRequest);
        }
        data.tokens -= amount;
        e.storage()
            .persistent()
            .set(&MockBackstopKey::Data(pool_address), &data);
        soroban_sdk::token::TokenClient::new(&e, &Self::backstop_token(e.clone())).transfer(
            &e.current_contract_address(),
            &to,
            &amount,
        );
    }

    pub fn donate(e: Env, from: Address, pool_address: Address, amount: i128) {
        from.require_auth();
        pool_address.require_auth();
        if amount < 0 {
            soroban_sdk::panic_with_error!(&e, MockBackstopError::BadRequest);
        }
        soroban_sdk::token::TokenClient::new(&e, &Self::backstop_token(e.clone())).transfer(
            &from,
            e.current_contract_address(),
            &amount,
        );
        let mut data = Self::pool_data(e.clone(), pool_address.clone());
        data.tokens += amount;
        e.storage()
            .persistent()
            .set(&MockBackstopKey::Data(pool_address), &data);
    }
}

/// Thin dynamic client for [`MockBackstopContract`] (same style as
/// [`MockOracleClient`]: keeps mocks out of the generated-client surface).
pub struct MockBackstopClient<'a> {
    env: &'a Env,
    address: Address,
}

impl<'a> MockBackstopClient<'a> {
    pub fn new(env: &'a Env, address: &Address) -> Self {
        Self {
            env,
            address: address.clone(),
        }
    }

    pub fn address(&self) -> &Address {
        &self.address
    }

    /// Sets the `pool_data` the mock reports for `pool`.
    pub fn set_pool_data(&self, pool: &Address, data: &PoolBackstopData) {
        let args = Vec::from_array(
            self.env,
            [
                pool.clone().into_val(self.env),
                data.clone().into_val(self.env),
            ],
        );
        let _: Val =
            self.env
                .invoke_contract(&self.address, &Symbol::new(self.env, "set_pool_data"), args);
    }

    /// The backstop token (SAC) the mock was constructed with.
    pub fn backstop_token(&self) -> Address {
        let v: Val = self.env.invoke_contract(
            &self.address,
            &Symbol::new(self.env, "backstop_token"),
            Vec::new(self.env),
        );
        v.try_into_val(self.env).unwrap()
    }
}

/********** Fixture **********/

/// Carry-adapter harness: fork pool stack (sha256-pinned wasm), two pools at
/// status 0 (admin active), a settable mock oracle with a call counter, and
/// third-party liquidity helpers on both pools.
///
/// Layout (per the carry plan):
///
/// - `pool`: collateral reserve (`collateral_token`, 90%/95% c/l-factor) +
///   borrow reserve (`borrow_token`).
/// - `yield_pool`: borrow reserve (`borrow_token`) with seed supply so the
///   adapter's yield leg can be supplied and withdrawn.
///
/// `admin` is the pool factory/creation admin; `whale` holds the seed pool
/// liquidity; `third_party` is the counterparty whose borrow/repay drives
/// accrual and utilization tests.
pub struct CarryFixture<'a> {
    pub env: Env,
    pub admin: Address,
    pub whale: Address,
    pub third_party: Address,
    pub collateral_token: Address,
    pub borrow_token: Address,
    pub blnd_token: Address,
    pub oracle: MockOracleClient<'a>,
    pub backstop: MockBackstopClient<'a>,
    pub pool_factory: ForkFactoryClient<'a>,
    pub pool: PoolClient<'a>,
    pub pool_collateral_idx: u32,
    pub pool_borrow_idx: u32,
    pub yield_pool: PoolClient<'a>,
    pub yield_pool_borrow_idx: u32,
}

impl<'a> CarryFixture<'a> {
    /// Builds the full harness: tokens -> mock backstop / emitter ->
    /// fork factory -> mock oracle -> both pools created, reserved and
    /// activated via the pool's genuine threshold/q4w `set_status` path.
    pub fn new(env: &'a Env) -> Self {
        env.mock_all_auths();
        env.cost_estimate().budget().reset_unlimited();

        let admin = Address::generate(env);
        let whale = Address::generate(env);
        let third_party = Address::generate(env);
        env.ledger().set_timestamp(1_713_139_200); // fork tests' activation-safe epoch

        // ---- SAC tokens ----
        // BLND admin becomes the emitter once it is created (mirrors the
        // stock BlendFixture / fork TestFixture). The carry assets carry the
        // same admin so `mint` stays explicit in tests.
        let blnd = env.register_stellar_asset_contract_v2(admin.clone());
        let collateral = env.register_stellar_asset_contract_v2(admin.clone());
        let borrow = env.register_stellar_asset_contract_v2(admin.clone());
        let blnd_token = blnd.address();
        let collateral_token = collateral.address();
        let borrow_token = borrow.address();

        // ---- backstop token (SAC), mock backstop (registered natively) and
        // emitter (stock wasm; owns BLND admin like the stock fixture) ----
        let backstop_token = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let backstop_id = env.register(MockBackstopContract {}, (&backstop_token,));
        let factory_id = Address::generate(env);
        let emitter_id = env.register(emitter_fixture::WASM, ());
        let emitter = emitter_fixture::Client::new(env, &emitter_id);
        emitter.initialize(&blnd_token, &backstop_id, &Address::generate(env));
        let sac_admin = soroban_sdk::token::StellarAssetClient::new(env, &blnd_token);
        sac_admin.set_admin(&emitter_id);

        // ---- real fork factory constructed with the uploaded fork pool hash ----
        let pool_hash = env.deployer().upload_contract_wasm(POOL_WASM);
        env.register_at(
            &factory_id,
            FACTORY_WASM,
            (PoolInitMeta {
                backstop: backstop_id.clone(),
                pool_hash,
                blnd_id: blnd_token.clone(),
            },),
        );
        let backstop = MockBackstopClient::new(env, &backstop_id);
        let pool_factory = ForkFactoryClient::new(env, &factory_id);

        // ---- mock oracle ----
        let oracle_id = env.register(MockOracleContract {}, ());
        let oracle = MockOracleClient::new(env, &oracle_id);
        oracle.set_data(
            &symbol_short!("USD"),
            &vec![
                env,
                blnd_token.clone(),
                collateral_token.clone(),
                borrow_token.clone(),
            ],
            &7,
            &300,
        );
        oracle.set_price(&vec![env, blnd_token.clone()], &1_000_000);
        oracle.set_price(&vec![env, borrow_token.clone()], &1_0000000);
        // collateral priced at 10_000 USD (BTC-like scale)
        oracle.set_price(&vec![env, collateral_token.clone()], &100_000_000_000);

        // ---- create and activate both pools ----
        let pool_addr = pool_factory.deploy(
            &admin,
            &String::from_str(env, "CarryCollateral"),
            &BytesN::random(env),
            &oracle_id,
            &0,         // backstop take rate
            &10,        // max positions
            &1_0000000, // min collateral
        );
        let yield_pool_addr = pool_factory.deploy(
            &admin,
            &String::from_str(env, "CarryYield"),
            &BytesN::random(env),
            &oracle_id,
            &0,
            &10,
            &1_0000000,
        );
        let pool = PoolClient::new(env, &pool_addr);
        let yield_pool = PoolClient::new(env, &yield_pool_addr);

        // Collateral pool reserves: collateral (90/95, cap = MAX) + borrow.
        let mut collateral_config = default_reserve_metadata();
        collateral_config.c_factor = 9_000_000;
        collateral_config.l_factor = 9_500_000;
        pool.queue_set_reserve(&collateral_token, &collateral_config);
        let pool_collateral_idx = pool.set_reserve(&collateral_token);
        let borrow_config = default_reserve_metadata();
        pool.queue_set_reserve(&borrow_token, &borrow_config);
        let pool_borrow_idx = pool.set_reserve(&borrow_token);

        // Yield pool reserve: borrow asset supply leg.
        yield_pool.queue_set_reserve(&borrow_token, &borrow_config);
        let yield_pool_borrow_idx = yield_pool.set_reserve(&borrow_token);

        // Activate both pools via the genuine path:
        // mock backstop data clears the threshold -> set_status(0).
        Self::seed_backstop_and_activate(env, &backstop, &pool);
        Self::seed_backstop_and_activate(env, &backstop, &yield_pool);

        // Seed the pools with liquidity so borrows have something to borrow:
        // whale supplies borrow token on both pools and collateral on `pool`.
        Self::mint(env, &borrow_token, &whale, 10_000_000_000_000);
        Self::mint(env, &collateral_token, &whale, 100_0000000);
        Self::mint(env, &borrow_token, &third_party, 100_0000000);
        Self::mint(env, &collateral_token, &third_party, 1_0000000);
        Self::submit(
            env,
            &pool,
            &whale,
            &collateral_token,
            50_0000000,
            RequestType::SupplyCollateral,
        );
        Self::submit(
            env,
            &pool,
            &whale,
            &borrow_token,
            5_000_000_000_000,
            RequestType::Supply,
        );
        Self::submit(
            env,
            &yield_pool,
            &whale,
            &borrow_token,
            5_000_000_000_000,
            RequestType::Supply,
        );

        Self {
            env: env.clone(),
            admin,
            whale,
            third_party,
            collateral_token,
            borrow_token,
            blnd_token,
            oracle,
            backstop,
            pool_factory,
            pool,
            pool_collateral_idx,
            pool_borrow_idx,
            yield_pool,
            yield_pool_borrow_idx,
        }
    }

    /// Mints `amount` of `token` to `to` via the SAC admin path
    /// (`mock_all_auths` covers the admin signature — BLND's admin is the
    /// emitter, the other tokens' admin is the fixture `admin`).
    pub fn mint(env: &Env, token: &Address, to: &Address, amount: i128) {
        env.mock_all_auths();
        soroban_sdk::token::StellarAssetClient::new(env, token).mint(to, &amount);
    }

    /// Submits a single `request_type` request for `amount` of `token` on
    /// `pool` where `from` is spender/took-own-position and tokens flow
    /// `from` -> `to`.
    fn submit(
        env: &Env,
        pool: &PoolClient,
        from: &Address,
        token: &Address,
        amount: i128,
        request_type: RequestType,
    ) {
        let request = Request {
            request_type: request_type as u32,
            address: token.clone(),
            amount,
        };
        pool.submit(from, from, from, &vec![env, request]);
    }

    fn seed_backstop_and_activate(env: &Env, backstop: &MockBackstopClient, pool: &PoolClient) {
        // Backstop data that genuinely clears the pool's threshold
        // (blnd^4 * usdc / 100k^5 >= 1 with unit-floored balances) with no q4w.
        // The backstop token is minted to the mock so `draw` stays solvent.
        let tokens = 500_000_000_000i128;
        Self::mint(env, &backstop.backstop_token(), &backstop.address, tokens);
        backstop.set_pool_data(
            &pool.address,
            &PoolBackstopData {
                tokens,
                shares: tokens,
                q4w_pct: 0,
                blnd: 2_000_000_000_000,
                usdc: 100_000_000_000,
                token_spot_price: 1_0000000,
            },
        );
        pool.set_status(&0);
        let status = pool.update_status();
        assert!(status <= 1, "pool activation must reach status <= 1");
    }

    /// Activates an extra pool deployed via `pool_factory.deploy(..)` through
    /// the genuine path: mints the backstop token to the mock, reports
    /// threshold-clearing pool data (blnd^4 * usdc / 100k^5 >= 1, q4w 0), then
    /// `set_status(0)` + `update_status()` on the real fork pool.
    pub fn seed_extra_pool(&self, pool: &PoolClient) {
        Self::seed_backstop_and_activate(&self.env, &self.backstop, pool);
    }

    /// Advances ledger time by `secs` and refreshes the mock oracle
    /// timestamps so pool oracle reads stay valid after the jump.
    pub fn advance_time(&self, secs: u64) {
        self.env
            .ledger()
            .set_timestamp(self.env.ledger().timestamp().saturating_add(secs));
        self.refresh_prices();
        // Update the borrow-leg reserves so accrual is applied on the next
        // store action (the fork only persists accrual inside submit).
        let _ = self.pool.get_reserve(&self.borrow_token);
        let _ = self.yield_pool.get_reserve(&self.borrow_token);
    }

    /// Re-stamps every fixture price with the current ledger timestamp
    /// (mirrors what a live feed does between pool interactions).
    pub fn refresh_prices(&self) {
        self.oracle
            .set_price(&vec![&self.env, self.borrow_token.clone()], &1_0000000);
        self.oracle.set_price(
            &vec![&self.env, self.collateral_token.clone()],
            &100_000_000_000,
        );
        self.oracle
            .set_price(&vec![&self.env, self.blnd_token.clone()], &1_000_000);
    }

    /// Sets the oracle price for `token` and re-stamps its freshness.
    pub fn set_oracle_price(&self, token: &Address, price: i128) {
        self.oracle
            .set_price(&vec![&self.env, token.clone()], &price);
    }

    /// Supplies `amount` of `token` from `from` (third-party liquidity; plain
    /// supply, or collateral on the collateral pool when `as_collateral`).
    /// `from` must already hold the tokens (mint via `CarryFixture::mint`).
    pub fn third_party_supply(
        &self,
        pool: &PoolClient,
        from: &Address,
        token: &Address,
        amount: i128,
        as_collateral: bool,
    ) {
        Self::submit(
            &self.env,
            pool,
            from,
            token,
            amount,
            if as_collateral {
                RequestType::SupplyCollateral
            } else {
                RequestType::Supply
            },
        );
    }

    /// Borrows `amount` of `token` from `pool` as `from`, tokens going to
    /// `to`. The caller must hold sufficient collateral (or accept the
    /// revert).
    pub fn third_party_borrow(
        &self,
        pool: &PoolClient,
        from: &Address,
        to: &Address,
        token: &Address,
        amount: i128,
    ) {
        let request = Request {
            request_type: RequestType::Borrow as u32,
            address: token.clone(),
            amount,
        };
        pool.submit(from, to, from, &vec![&self.env, request]);
    }

    /// Repays `amount` of `token` on `pool` as `from`. `from` must already
    /// hold the tokens (mint via `CarryFixture::mint`).
    pub fn third_party_repay(
        &self,
        pool: &PoolClient,
        from: &Address,
        token: &Address,
        amount: i128,
    ) {
        Self::submit(&self.env, pool, from, token, amount, RequestType::Repay);
    }

    /// The `Reserve` snapshot for `token` on `pool` (d_rate/b_rate/utilization
    /// for accrual assertions).
    pub fn reserve(&self, pool: &PoolClient, token: &Address) -> Reserve {
        pool.get_reserve(token)
    }

    /// The raw `Positions` for `who` on `pool`.
    pub fn positions(&self, pool: &PoolClient, who: &Address) -> Positions {
        pool.get_positions(who)
    }
}
