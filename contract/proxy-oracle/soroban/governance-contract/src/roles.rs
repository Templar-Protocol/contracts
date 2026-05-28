//! Role membership, backed by the audited `stellar-access` crate from
//! `OpenZeppelin`.
//!
//! Authorization is enforced by this contract (see `engine::require_authorized`)
//! before any state change, so all mutations go through the `*_no_auth`
//! variants. We deliberately do not use stellar-access's single overarching
//! admin: our `Admin` is a multi-member role with last-member protection, which
//! maps onto a named role plus a `get_role_member_count` guard.

use soroban_sdk::{Address, Env, Symbol, Vec};
use stellar_access::access_control;
use templar_proxy_oracle_soroban_common::{GovernanceError, Role};

fn role_symbol(env: &Env, role: Role) -> Symbol {
    match role {
        Role::Admin => Symbol::new(env, "Admin"),
        Role::ManualTripper => Symbol::new(env, "ManualTripper"),
        Role::CircuitBreakerOperator => Symbol::new(env, "CircuitBreakerOperator"),
        Role::ProxyConfigurationManager => Symbol::new(env, "ProxyConfigurationManager"),
    }
}

pub fn has_role(env: &Env, account: &Address, role: Role) -> bool {
    access_control::has_role(env, account, &role_symbol(env, role)).is_some()
}

pub fn grant(env: &Env, account: &Address, role: Role, caller: &Address) {
    access_control::grant_role_no_auth(env, account, &role_symbol(env, role), caller);
}

/// Revokes `role` from `account`. Granting/revoking is idempotent (revoking a
/// role the account doesn't hold is a no-op), and the last `Admin` cannot be
/// removed — mirroring the previous hand-rolled behavior.
pub fn revoke(
    env: &Env,
    account: &Address,
    role: Role,
    caller: &Address,
) -> Result<(), GovernanceError> {
    let symbol = role_symbol(env, role);
    if access_control::has_role(env, account, &symbol).is_none() {
        return Ok(());
    }
    if role == Role::Admin && access_control::get_role_member_count(env, &symbol) <= 1 {
        return Err(GovernanceError::LastAdmin);
    }
    access_control::revoke_role_no_auth(env, account, &symbol, caller);
    Ok(())
}

pub fn members(env: &Env, role: Role) -> Vec<Address> {
    let symbol = role_symbol(env, role);
    let count = access_control::get_role_member_count(env, &symbol);
    let mut out = Vec::new(env);
    let mut index = 0;
    while index < count {
        out.push_back(access_control::get_role_member(env, &symbol, index));
        index += 1;
    }
    out
}

pub fn roles_of(env: &Env, account: &Address) -> Vec<Role> {
    let mut out = Vec::new(env);
    for role in Role::ALL {
        if has_role(env, account, role) {
            out.push_back(role);
        }
    }
    out
}
