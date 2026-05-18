#![no_std]

mod types;
pub use types::*;

use soroban_sdk::{contract, contractimpl, panic_with_error, Address, Env, MuxedAddress, String};
use stellar_tokens::fungible::{
    burnable::{emit_burn, FungibleBurnable},
    Base, FungibleToken,
};

const INSTANCE_TTL_THRESHOLD: u32 = 518_400;
const INSTANCE_TTL_EXTEND_TO: u32 = 3_110_400;

#[contract]
pub struct SorobanShareTokenContract;

// ── SEP-41 core (delegates to OZ Base) ───────────────────────

#[contractimpl]
impl FungibleToken for SorobanShareTokenContract {
    type ContractType = Base;

    fn total_supply(e: &Env) -> i128 {
        Base::total_supply(e)
    }

    fn balance(e: &Env, account: Address) -> i128 {
        Base::balance(e, &account)
    }

    fn allowance(e: &Env, owner: Address, spender: Address) -> i128 {
        Base::allowance(e, &owner, &spender)
    }

    fn transfer(e: &Env, from: Address, to: MuxedAddress, amount: i128) {
        extend_instance_ttl(e);
        Base::transfer(e, &from, &to, amount);
    }

    fn transfer_from(e: &Env, spender: Address, from: Address, to: Address, amount: i128) {
        extend_instance_ttl(e);
        Base::transfer_from(e, &spender, &from, &to, amount);
    }

    fn approve(e: &Env, owner: Address, spender: Address, amount: i128, live_until_ledger: u32) {
        extend_instance_ttl(e);
        Base::approve(e, &owner, &spender, amount, live_until_ledger);
    }

    fn decimals(e: &Env) -> u32 {
        Base::decimals(e)
    }

    fn name(e: &Env) -> String {
        Base::name(e)
    }

    fn symbol(e: &Env) -> String {
        Base::symbol(e)
    }
}

// ── SEP-41 burnable (vault-gated override) ───────────────────

#[contractimpl]
impl FungibleBurnable for SorobanShareTokenContract {
    fn burn(e: &Env, from: Address, amount: i128) {
        extend_instance_ttl(e);
        require_vault_invoker(e);
        Base::update(e, Some(&from), None, amount);
        emit_burn(e, &from, amount);
    }

    fn burn_from(e: &Env, spender: Address, from: Address, amount: i128) {
        extend_instance_ttl(e);
        require_vault_invoker(e);
        Base::spend_allowance(e, &from, &spender, amount);
        Base::update(e, Some(&from), None, amount);
        emit_burn(e, &from, amount);
    }
}

// ── Vault share-token extras ─────────────────────────────────

#[contractimpl]
impl SorobanShareTokenContract {
    pub fn __constructor(
        env: Env,
        admin: Address,
        vault: Address,
        name: String,
        symbol: String,
        decimals: u32,
    ) {
        extend_instance_ttl(&env);
        require_contract_address(&env, &vault);
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Vault, &vault);
        Base::set_metadata(&env, decimals, name, symbol);
    }

    pub fn mint(env: Env, to: Address, amount: i128) {
        extend_instance_ttl(&env);
        require_vault_invoker(&env);
        Base::mint(&env, &to, amount);
    }

    pub fn set_admin(env: Env, caller: Address, admin: Address) {
        extend_instance_ttl(&env);
        require_admin(&env, &caller);
        env.storage().instance().set(&DataKey::Admin, &admin);
    }

    pub fn set_vault(env: Env, caller: Address, vault: Address) {
        extend_instance_ttl(&env);
        require_admin(&env, &caller);
        require_contract_address(&env, &vault);
        env.storage().instance().set(&DataKey::Vault, &vault);
    }

    pub fn set_metadata(env: Env, caller: Address, _name: String, _symbol: String, _decimals: u32) {
        extend_instance_ttl(&env);
        require_admin(&env, &caller);
        panic_with_error!(&env, ShareTokenError::MetadataImmutable);
    }

    pub fn admin(env: Env) -> Address {
        extend_instance_ttl(&env);
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| panic_with_error!(&env, ShareTokenError::MissingConfig))
    }

    pub fn vault(env: Env) -> Address {
        extend_instance_ttl(&env);
        env.storage()
            .instance()
            .get(&DataKey::Vault)
            .unwrap_or_else(|| panic_with_error!(&env, ShareTokenError::MissingConfig))
    }

    pub fn extend_ttl(env: Env, caller: Address) {
        require_admin(&env, &caller);
        extend_instance_ttl(&env);
    }
}

// ── Helpers ──────────────────────────────────────────────────

fn require_admin(env: &Env, caller: &Address) {
    caller.require_auth();
    let admin: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .unwrap_or_else(|| panic_with_error!(env, ShareTokenError::MissingConfig));
    if caller != &admin {
        panic_with_error!(env, ShareTokenError::Unauthorized);
    }
}

fn require_vault_invoker(env: &Env) {
    let vault: Address = env
        .storage()
        .instance()
        .get(&DataKey::Vault)
        .unwrap_or_else(|| panic_with_error!(env, ShareTokenError::MissingConfig));
    vault.require_auth();
}

fn is_contract_address(addr: &Address) -> bool {
    let bytes = addr.to_string().to_bytes();
    matches!(bytes.get(0), Some(b'C'))
}

fn require_contract_address(env: &Env, addr: &Address) {
    if !is_contract_address(addr) {
        panic_with_error!(env, ShareTokenError::InvalidInput);
    }
}

fn extend_instance_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(INSTANCE_TTL_THRESHOLD, INSTANCE_TTL_EXTEND_TO);
}

#[cfg(test)]
mod tests;
