#![no_std]

extern crate alloc;

#[cfg(test)]
extern crate std;

use soroban_sdk::Address;

mod contract;
mod error;

pub(crate) type ProxyCoreView = (
    (Address, Address, Address, Address),
    (i128, i128, bool),
    (i128, i128, i128, i128),
    (i128, u64, i128, i128),
);
pub(crate) type ProxyPolicyView = (
    soroban_sdk::Vec<u32>,
    soroban_sdk::Vec<(soroban_sdk::String, i128, i128)>,
);
pub(crate) type ProxyPreviewView = (i128, i128, i128, i128, i128, i128, i128, i128);
pub(crate) type ProxyViewResponse = (ProxyCoreView, ProxyPolicyView, ProxyPreviewView);

pub use {
    contract::Soroban4626ProxyContract, error::ContractError,
    templar_soroban_shared_types::VaultCommandResult,
};

#[cfg(test)]
mod tests;
