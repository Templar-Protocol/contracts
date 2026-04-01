#![no_std]

use soroban_sdk::contracttype;

#[contracttype]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum GovernanceConfigKind {
    Curator,
    Governance,
    Sentinel,
    Guardians,
    Allocators,
    AllowedAdapters,
    SkimRecipient,
    VirtualOffsets,
}

#[contracttype]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum GovernancePolicyKind {
    SupplyQueue,
    Cap,
    RemoveMarket,
    Restrictions,
    Group,
    Paused,
    Fees,
}
