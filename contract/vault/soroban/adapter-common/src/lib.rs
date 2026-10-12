//! Admin / pause / TTL / upgrade helpers shared by Soroban vault adapters.
//!
//! Free functions only: each adapter keeps its own `#[contractimpl]` surface
//! and error enum, and maps [`CommonError`] into it.
#![no_std]

use soroban_sdk::{
    address_payload::AddressPayload, contracttype, symbol_short, Address, BytesN, Env,
};
use stellar_contract_utils::upgradeable;

/// Re-extend instance storage TTL when remaining TTL drops below ~30 days.
pub const INSTANCE_TTL_THRESHOLD: u32 = 518_400;
/// Extend instance storage TTL to the Soroban maximum (~6 months).
pub const INSTANCE_TTL_EXTEND_TO: u32 = 3_110_400;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum CommonError {
    Unauthorized,
    MissingConfig,
    Paused,
    NotContract,
}

#[contracttype]
#[derive(Clone, Copy)]
pub enum CommonKey {
    Admin,
    PendingAdmin,
    Vault,
    Paused,
}

pub fn extend_instance_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(INSTANCE_TTL_THRESHOLD, INSTANCE_TTL_EXTEND_TO);
}

pub fn init_admin_vault(env: &Env, admin: &Address, vault: &Address) {
    let storage = env.storage().instance();
    storage.set(&CommonKey::Admin, admin);
    storage.set(&CommonKey::Vault, vault);
}

fn get_address(env: &Env, key: CommonKey) -> Result<Address, CommonError> {
    env.storage()
        .instance()
        .get(&key)
        .ok_or(CommonError::MissingConfig)
}

pub fn admin(env: &Env) -> Result<Address, CommonError> {
    get_address(env, CommonKey::Admin)
}

pub fn vault(env: &Env) -> Result<Address, CommonError> {
    get_address(env, CommonKey::Vault)
}

pub fn pending_admin(env: &Env) -> Result<Address, CommonError> {
    get_address(env, CommonKey::PendingAdmin)
}

pub fn require_admin(env: &Env, caller: &Address) -> Result<(), CommonError> {
    caller.require_auth();
    if caller != &admin(env)? {
        return Err(CommonError::Unauthorized);
    }
    Ok(())
}

pub fn require_vault(env: &Env, caller: &Address) -> Result<(), CommonError> {
    caller.require_auth();
    if caller != &vault(env)? {
        return Err(CommonError::Unauthorized);
    }
    Ok(())
}

/// Caller must be the admin or the vault (blend-adapter's pause policy).
pub fn require_admin_or_vault(env: &Env, caller: &Address) -> Result<(), CommonError> {
    caller.require_auth();
    if caller != &admin(env)? && caller != &vault(env)? {
        return Err(CommonError::Unauthorized);
    }
    Ok(())
}

/// Set the pause flag and publish the `paused` event (caller already authorized).
#[allow(deprecated)]
pub fn set_paused_flag(env: &Env, caller: &Address, paused: bool) {
    env.storage().instance().set(&CommonKey::Paused, &paused);
    env.events()
        .publish((symbol_short!("paused"), caller.clone()), paused);
}

pub fn is_paused(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&CommonKey::Paused)
        .unwrap_or(false)
}

pub fn require_not_paused(env: &Env) -> Result<(), CommonError> {
    if is_paused(env) {
        return Err(CommonError::Paused);
    }
    Ok(())
}

/// Admin-only: stage `new_admin` as pending (two-step handover).
#[allow(deprecated)]
pub fn propose_admin(env: &Env, caller: &Address, new_admin: &Address) -> Result<(), CommonError> {
    require_admin(env, caller)?;
    env.storage()
        .instance()
        .set(&CommonKey::PendingAdmin, new_admin);
    env.events().publish(
        (symbol_short!("admin_set"), caller.clone()),
        new_admin.clone(),
    );
    Ok(())
}

/// Pending admin accepts: becomes admin and the pending slot is cleared.
/// Emits `(admin_acc, old_admin)` with the new admin as data, like blend-adapter.
#[allow(deprecated)]
pub fn accept_admin(env: &Env, caller: &Address) -> Result<(), CommonError> {
    caller.require_auth();
    if caller != &pending_admin(env)? {
        return Err(CommonError::Unauthorized);
    }
    let old_admin = admin(env)?;
    let storage = env.storage().instance();
    storage.set(&CommonKey::Admin, caller);
    storage.remove(&CommonKey::PendingAdmin);
    env.events()
        .publish((symbol_short!("admin_acc"), old_admin), caller.clone());
    Ok(())
}

/// Admin-only wasm upgrade; emits `upgrade`.
#[allow(deprecated)]
pub fn upgrade_as_admin(
    env: &Env,
    wasm_hash: &BytesN<32>,
    operator: &Address,
) -> Result<(), CommonError> {
    require_admin(env, operator)?;
    upgradeable::upgrade(env, wasm_hash);
    env.events().publish(
        (symbol_short!("upgrade"), operator.clone()),
        wasm_hash.clone(),
    );
    Ok(())
}

pub fn is_contract_address(addr: &Address) -> bool {
    matches!(
        AddressPayload::from_address(addr),
        Some(AddressPayload::ContractIdHash(_))
    )
}

pub fn require_contract_address(addr: &Address) -> Result<(), CommonError> {
    if is_contract_address(addr) {
        Ok(())
    } else {
        Err(CommonError::NotContract)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{contract, contractimpl, testutils::Address as _};

    #[contract]
    struct Dummy;
    #[contractimpl]
    impl Dummy {}

    #[test]
    fn two_step_admin_and_guards() {
        let env = Env::default();
        env.mock_all_auths();
        let id = env.register(Dummy, ());
        let (admin_a, vault_a, next, other) = (
            Address::generate(&env),
            Address::generate(&env),
            Address::generate(&env),
            Address::generate(&env),
        );
        // Each auth-consuming call runs in its own host frame (one host
        // function invocation) so the mock auth records do not collide.
        env.as_contract(&id, || init_admin_vault(&env, &admin_a, &vault_a));
        env.as_contract(&id, || {
            assert_eq!(require_admin(&env, &other), Err(CommonError::Unauthorized));
        });
        env.as_contract(&id, || {
            assert_eq!(require_vault(&env, &other), Err(CommonError::Unauthorized));
        });
        env.as_contract(&id, || {
            assert_eq!(require_admin(&env, &admin_a), Ok(()));
            assert_eq!(require_vault(&env, &vault_a), Ok(()));
        });
        env.as_contract(&id, || {
            assert_eq!(accept_admin(&env, &next), Err(CommonError::MissingConfig));
        });
        env.as_contract(&id, || {
            propose_admin(&env, &admin_a, &next).unwrap();
        });
        env.as_contract(&id, || {
            assert_eq!(accept_admin(&env, &other), Err(CommonError::Unauthorized));
        });
        env.as_contract(&id, || {
            accept_admin(&env, &next).unwrap();
            assert_eq!(admin(&env), Ok(next));
            assert_eq!(pending_admin(&env), Err(CommonError::MissingConfig));
            assert_eq!(require_contract_address(&id), Ok(()));
            let account = Address::from_str(
                &env,
                "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            );
            assert!(
                !is_contract_address(&account),
                "plain account must not pass contract-address check"
            );
            assert_eq!(
                require_contract_address(&account),
                Err(CommonError::NotContract)
            );
        });
    }
}
