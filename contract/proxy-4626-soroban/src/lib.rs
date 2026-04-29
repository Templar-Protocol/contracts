#![no_std]

extern crate alloc;

#[cfg(test)]
extern crate std;

mod contract;
mod error;

pub use {
    contract::Soroban4626ProxyContract,
    error::ContractError,
    templar_soroban_shared_types::{
        ProxyPreviewFields, ProxyPreviewView, ProxyViewFields, ProxyViewResponse,
        VaultCommandResult,
    },
};

#[cfg(test)]
mod tests;
