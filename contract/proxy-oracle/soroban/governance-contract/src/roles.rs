//! Bespoke RBAC for governance proposals.
//!
//! Storage is a single `Vec<Address>` per role — membership lookup, listing,
//! and "roles for an account" are all derived from that single source of truth.
//! `Role::Admin` is treated as a global override that satisfies any role check.

use soroban_sdk::{contracttype, Address, Env, Vec};
use templar_proxy_oracle_soroban_common::{GovernanceError, Role};

#[contracttype]
#[derive(Clone)]
enum RoleKey {
    Members(Role),
}

pub fn members(env: &Env, role: Role) -> Vec<Address> {
    env.storage()
        .instance()
        .get(&RoleKey::Members(role))
        .unwrap_or_else(|| Vec::new(env))
}

pub fn has(env: &Env, account: &Address, role: Role) -> bool {
    members(env, role).iter().any(|m| &m == account)
}

pub fn grant(env: &Env, account: &Address, role: Role) {
    let mut members = members(env, role);
    if !members.iter().any(|m| &m == account) {
        members.push_back(account.clone());
        save(env, role, &members);
    }
}

pub fn revoke(env: &Env, account: &Address, role: Role) -> Result<(), GovernanceError> {
    let mut members = members(env, role);
    let Some(index) = members
        .iter()
        .position(|m| &m == account)
        .and_then(|i| u32::try_from(i).ok())
    else {
        return Ok(());
    };
    if role == Role::Admin && members.len() <= 1 {
        return Err(GovernanceError::LastAdmin);
    }
    members.remove(index);
    save(env, role, &members);
    Ok(())
}

pub fn roles_of(env: &Env, account: &Address) -> Vec<Role> {
    let mut roles = Vec::new(env);
    for role in Role::ALL {
        if has(env, account, role) {
            roles.push_back(role);
        }
    }
    roles
}

fn save(env: &Env, role: Role, members: &Vec<Address>) {
    env.storage()
        .instance()
        .set(&RoleKey::Members(role), members);
}
