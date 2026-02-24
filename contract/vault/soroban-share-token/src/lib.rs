#![no_std]

mod types;
pub use types::*;

use soroban_sdk::{contract, contractimpl, Address, Env, String};

const INSTANCE_TTL_THRESHOLD: u32 = 518_400;
const INSTANCE_TTL_EXTEND_TO: u32 = 3_110_400;
const BALANCE_TTL_THRESHOLD: u32 = 501_120;
const BALANCE_TTL_EXTEND_TO: u32 = 518_400;

#[contract]
pub struct SorobanShareTokenContract;

#[contractimpl]
impl SorobanShareTokenContract {
    pub fn __constructor(
        env: Env,
        admin: Address,
        vault: Address,
        name: String,
        symbol: String,
        decimals: u32,
    ) -> Result<(), ShareTokenError> {
        extend_instance_ttl(&env);
        require_contract_address(&vault)?;

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Vault, &vault);
        env.storage().instance().set(&DataKey::Name, &name);
        env.storage().instance().set(&DataKey::Symbol, &symbol);
        env.storage().instance().set(&DataKey::Decimals, &decimals);
        env.storage().instance().set(&DataKey::TotalSupply, &0i128);
        Ok(())
    }

    pub fn set_admin(env: Env, caller: Address, admin: Address) -> Result<(), ShareTokenError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        env.storage().instance().set(&DataKey::Admin, &admin);
        Ok(())
    }

    pub fn set_vault(env: Env, caller: Address, vault: Address) -> Result<(), ShareTokenError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        require_contract_address(&vault)?;
        env.storage().instance().set(&DataKey::Vault, &vault);
        Ok(())
    }

    pub fn set_metadata(
        env: Env,
        caller: Address,
        name: String,
        symbol: String,
        decimals: u32,
    ) -> Result<(), ShareTokenError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        env.storage().instance().set(&DataKey::Name, &name);
        env.storage().instance().set(&DataKey::Symbol, &symbol);
        env.storage().instance().set(&DataKey::Decimals, &decimals);
        Ok(())
    }

    pub fn mint(env: Env, to: Address, amount: i128) -> Result<(), ShareTokenError> {
        extend_instance_ttl(&env);
        if amount <= 0 {
            return Err(ShareTokenError::InvalidInput);
        }
        require_vault_invoker(&env)?;

        let total_supply = total_supply_raw(&env);
        let next_total = total_supply
            .checked_add(amount)
            .ok_or(ShareTokenError::ArithmeticOverflow)?;
        env.storage()
            .instance()
            .set(&DataKey::TotalSupply, &next_total);

        increase_balance(&env, &to, amount)?;
        Mint {
            to: to.clone(),
            amount,
        }
        .publish(&env);
        Ok(())
    }

    pub fn burn(env: Env, from: Address, amount: i128) -> Result<(), ShareTokenError> {
        extend_instance_ttl(&env);
        if amount <= 0 {
            return Err(ShareTokenError::InvalidInput);
        }
        require_vault_invoker(&env)?;

        decrease_balance(&env, &from, amount)?;

        let total_supply = total_supply_raw(&env);
        let next_total = total_supply
            .checked_sub(amount)
            .ok_or(ShareTokenError::ArithmeticOverflow)?;
        env.storage()
            .instance()
            .set(&DataKey::TotalSupply, &next_total);

        Burn { from, amount }.publish(&env);
        Ok(())
    }

    pub fn transfer(
        env: Env,
        from: Address,
        to: Address,
        amount: i128,
    ) -> Result<(), ShareTokenError> {
        extend_instance_ttl(&env);
        if amount <= 0 {
            return Err(ShareTokenError::InvalidInput);
        }
        require_vault_invoker(&env)?;

        decrease_balance(&env, &from, amount)?;
        increase_balance(&env, &to, amount)?;

        Transfer { from, to, amount }.publish(&env);
        Ok(())
    }

    pub fn balance(env: Env, owner: Address) -> i128 {
        extend_instance_ttl(&env);
        balance_raw(&env, &owner)
    }

    pub fn total_supply(env: Env) -> i128 {
        extend_instance_ttl(&env);
        total_supply_raw(&env)
    }

    pub fn name(env: Env) -> Result<String, ShareTokenError> {
        extend_instance_ttl(&env);
        env.storage()
            .instance()
            .get(&DataKey::Name)
            .ok_or(ShareTokenError::MissingConfig)
    }

    pub fn symbol(env: Env) -> Result<String, ShareTokenError> {
        extend_instance_ttl(&env);
        env.storage()
            .instance()
            .get(&DataKey::Symbol)
            .ok_or(ShareTokenError::MissingConfig)
    }

    pub fn decimals(env: Env) -> Result<u32, ShareTokenError> {
        extend_instance_ttl(&env);
        env.storage()
            .instance()
            .get(&DataKey::Decimals)
            .ok_or(ShareTokenError::MissingConfig)
    }

    pub fn admin(env: Env) -> Result<Address, ShareTokenError> {
        extend_instance_ttl(&env);
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(ShareTokenError::MissingConfig)
    }

    pub fn vault(env: Env) -> Result<Address, ShareTokenError> {
        extend_instance_ttl(&env);
        env.storage()
            .instance()
            .get(&DataKey::Vault)
            .ok_or(ShareTokenError::MissingConfig)
    }

    pub fn extend_ttl(env: Env, caller: Address) -> Result<(), ShareTokenError> {
        require_admin(&env, &caller)?;
        extend_instance_ttl(&env);
        Ok(())
    }
}

fn require_admin(env: &Env, caller: &Address) -> Result<(), ShareTokenError> {
    caller.require_auth();
    let admin: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .ok_or(ShareTokenError::MissingConfig)?;
    if caller != &admin {
        return Err(ShareTokenError::Unauthorized);
    }
    Ok(())
}

fn require_vault_invoker(env: &Env) -> Result<(), ShareTokenError> {
    let vault: Address = env
        .storage()
        .instance()
        .get(&DataKey::Vault)
        .ok_or(ShareTokenError::MissingConfig)?;
    vault.require_auth();
    Ok(())
}

fn balance_raw(env: &Env, owner: &Address) -> i128 {
    let key = DataKey::Balance(owner.clone());
    if let Some(balance) = env.storage().persistent().get::<_, i128>(&key) {
        env.storage()
            .persistent()
            .extend_ttl(&key, BALANCE_TTL_THRESHOLD, BALANCE_TTL_EXTEND_TO);
        balance
    } else {
        0
    }
}

fn increase_balance(env: &Env, owner: &Address, amount: i128) -> Result<(), ShareTokenError> {
    let current = balance_raw(env, owner);
    let next = current
        .checked_add(amount)
        .ok_or(ShareTokenError::ArithmeticOverflow)?;
    let key = DataKey::Balance(owner.clone());
    env.storage().persistent().set(&key, &next);
    env.storage()
        .persistent()
        .extend_ttl(&key, BALANCE_TTL_THRESHOLD, BALANCE_TTL_EXTEND_TO);
    Ok(())
}

fn decrease_balance(env: &Env, owner: &Address, amount: i128) -> Result<(), ShareTokenError> {
    let current = balance_raw(env, owner);
    if current < amount {
        return Err(ShareTokenError::InsufficientBalance);
    }
    let next = current
        .checked_sub(amount)
        .ok_or(ShareTokenError::ArithmeticOverflow)?;
    let key = DataKey::Balance(owner.clone());
    env.storage().persistent().set(&key, &next);
    env.storage()
        .persistent()
        .extend_ttl(&key, BALANCE_TTL_THRESHOLD, BALANCE_TTL_EXTEND_TO);
    Ok(())
}

fn total_supply_raw(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::TotalSupply)
        .unwrap_or(0)
}

fn is_contract_address(addr: &Address) -> bool {
    let bytes = addr.to_string().to_bytes();
    matches!(bytes.get(0), Some(b'C'))
}

fn require_contract_address(addr: &Address) -> Result<(), ShareTokenError> {
    if is_contract_address(addr) {
        Ok(())
    } else {
        Err(ShareTokenError::InvalidInput)
    }
}

fn extend_instance_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(INSTANCE_TTL_THRESHOLD, INSTANCE_TTL_EXTEND_TO);
}

#[cfg(test)]
mod tests;
