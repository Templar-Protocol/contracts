use near_sdk::{assert_one_yocto, env, near, require, AccountId, AccountIdRef, NearToken, Promise};
use near_sdk_contract_tools::{
    hook::Hook,
    owner::OwnerInternal,
    slot::Slot,
    standard::nep145::{
        Nep145Controller, Nep145ControllerInternal, Nep145ForceUnregister, StorageBalance,
        StorageBalanceBounds,
    },
};
use templar_common::UnwrapReject;

#[cfg(test)]
mod solvency_tests;

use crate::{state::StorageKey, Contract, ContractExt};

impl Nep145ControllerInternal for Contract {
    type ForceUnregisterHook = Self;

    fn slot_account(account_id: &AccountIdRef) -> Slot<StorageBalance> {
        Slot::new(StorageKey::StorageAccount {
            hash: env::sha256_array(account_id.as_bytes()),
        })
    }

    fn slot_balance_bounds() -> Slot<StorageBalanceBounds> {
        Slot::new(StorageKey::StorageBounds)
    }
}

impl Hook<Contract, Nep145ForceUnregister<'_>> for Contract {
    fn hook<R>(
        _contract: &mut Contract,
        _action: &Nep145ForceUnregister<'_>,
        _f: impl FnOnce(&mut Contract) -> R,
    ) -> R {
        env::panic_str("Forced storage unregistration is not supported")
    }
}

impl Contract {
    /// Called only when initializing a fresh ledger, including the V1 migration.
    pub(crate) fn initialize_storage_management() -> u64 {
        // Hashed account keys and fixed-width balances make this minimum independent
        // of the account name. The bounds record itself is registry-funded.
        let mut account: Slot<StorageBalance> =
            Slot::new(StorageKey::StorageAccount { hash: [0; 32] });
        require!(!account.exists(), "Storage measurement slot is occupied");
        let before = env::storage_usage();
        account.write(&StorageBalance::default());
        let bytes = env::storage_usage()
            .checked_sub(before)
            .expect_or_reject("Invariant violation: storage registration did not grow storage");
        account.remove();
        let min = env::storage_byte_cost()
            .checked_mul(u128::from(bytes))
            .expect_or_reject("Storage registration cost overflow");
        <Self as Nep145ControllerInternal>::slot_balance_bounds()
            .write(&StorageBalanceBounds { min, max: None });
        let key_bytes = u64::try_from(account.key.len()).unwrap_or_reject();
        let balance_bytes = u64::try_from(
            near_sdk::borsh::object_length(&StorageBalance::default()).unwrap_or_reject(),
        )
        .unwrap_or_reject();
        bytes
            .checked_sub(key_bytes)
            .and_then(|bytes| bytes.checked_sub(balance_bytes))
            .expect_or_reject("Invariant violation: storage entry overhead")
    }

    /// Lock a registered payer's available balance for retained registry data.
    pub(crate) fn charge_storage(&mut self, account_id: &AccountId, amount: NearToken) {
        self.lock_storage(account_id, amount)
            .unwrap_or_else(|error| env::panic_str(&error.to_string()));
        let available = self
            .available_storage_balance
            .checked_sub(amount)
            .expect_or_reject("Invariant violation: aggregate storage balance underflow");
        self.available_storage_balance = available;
    }

    /// Credit freed data to its original payer without unlocking registration.
    pub(crate) fn release_storage(&mut self, account_id: &AccountId, amount: NearToken) {
        let balance = self
            .get_storage_balance(account_id)
            .unwrap_or_else(|error| env::panic_str(&error.to_string()));
        let releasable = balance
            .total
            .checked_sub(balance.available)
            .and_then(|locked| locked.checked_sub(self.get_storage_balance_bounds().min))
            .expect_or_reject("Invariant violation: registration storage is not locked");
        require!(
            amount <= releasable,
            "Cannot release registration storage balance"
        );
        let available = self
            .available_storage_balance
            .checked_add(amount)
            .expect_or_reject("Aggregate storage balance overflow");
        self.unlock_storage(account_id, amount)
            .unwrap_or_else(|error| env::panic_str(&error.to_string()));
        self.available_storage_balance = available;
    }

    /// Reserve still-unallocated owner/proposed-owner capacity without replacing Owner's derive.
    fn owner_storage_reserve(&self) -> NearToken {
        let maximum_value_bytes =
            u64::try_from(std::mem::size_of::<u32>() + AccountId::MAX_LEN).unwrap_or_reject();
        let bytes = [Self::slot_owner(), Self::slot_proposed_owner()]
            .into_iter()
            .map(|slot| match env::storage_read(&slot.key) {
                Some(value) => maximum_value_bytes
                    .checked_sub(u64::try_from(value.len()).unwrap_or_reject())
                    .expect_or_reject("Invariant violation: oversized ownership slot"),
                None => {
                    u64::try_from(slot.key.len()).unwrap_or_reject()
                        + maximum_value_bytes
                        + self.storage_entry_overhead
                }
            })
            .sum::<u64>();
        env::storage_byte_cost()
            .checked_mul(u128::from(bytes))
            .expect_or_reject("Owner storage reserve overflow")
    }

    /// Registry-funded writes and transfers must not spend user storage credits.
    pub(crate) fn assert_storage_solvency(&self, outgoing: NearToken) {
        let required = env::storage_byte_cost()
            .checked_mul(u128::from(env::storage_usage()))
            .and_then(|storage| storage.checked_add(self.available_storage_balance))
            .and_then(|storage| storage.checked_add(self.pending_deployment_storage))
            .and_then(|storage| storage.checked_add(self.owner_storage_reserve()))
            .expect_or_reject("Storage liability overflow");
        let remaining = env::account_balance()
            .checked_sub(outgoing)
            .expect_or_reject("Insufficient registry balance for transfer");
        require!(
            remaining >= required,
            "Insufficient registry balance to cover storage liabilities"
        );
    }
}

#[near]
impl Contract {
    /// Register or fund an account. Refunds always go to the depositing predecessor.
    #[payable]
    pub fn storage_deposit(
        &mut self,
        account_id: Option<AccountId>,
        registration_only: Option<bool>,
    ) -> StorageBalance {
        let predecessor = env::predecessor_account_id();
        let account_id = account_id.unwrap_or_else(|| predecessor.clone());
        let deposit = env::attached_deposit();
        let existing = self.get_storage_balance(&account_id).ok();
        let registration_only = registration_only.unwrap_or(false);
        if let Some(balance) = existing.as_ref().filter(|_| registration_only) {
            self.assert_storage_solvency(deposit);
            if !deposit.is_zero() {
                Promise::new(predecessor).transfer(deposit).detach();
            }
            return balance.clone();
        }

        let minimum = self.get_storage_balance_bounds().min;
        let credited = if registration_only { minimum } else { deposit };
        require!(deposit >= credited, "Insufficient storage deposit");
        let available_increase = if existing.is_none() {
            credited
                .checked_sub(minimum)
                .expect_or_reject("Insufficient storage deposit")
        } else {
            credited
        };
        let aggregate = self
            .available_storage_balance
            .checked_add(available_increase)
            .expect_or_reject("Aggregate storage balance overflow");
        let mut balance = self
            .deposit_to_storage_account(&account_id, credited)
            .unwrap_or_else(|error| env::panic_str(&error.to_string()));
        if existing.is_none() {
            balance = self
                .lock_storage(&account_id, minimum)
                .unwrap_or_else(|error| env::panic_str(&error.to_string()));
        }
        self.available_storage_balance = aggregate;
        let refund = deposit
            .checked_sub(credited)
            .expect_or_reject("Storage refund underflow");
        self.assert_storage_solvency(refund);
        if !refund.is_zero() {
            Promise::new(predecessor).transfer(refund).detach();
        }
        balance
    }

    #[payable]
    pub fn storage_withdraw(&mut self, amount: Option<NearToken>) -> StorageBalance {
        assert_one_yocto();
        let account_id = env::predecessor_account_id();
        let balance = self
            .get_storage_balance(&account_id)
            .unwrap_or_else(|error| env::panic_str(&error.to_string()));
        let amount = amount.unwrap_or(balance.available);
        require!(
            amount <= balance.available,
            "Insufficient available storage balance"
        );
        let aggregate = self
            .available_storage_balance
            .checked_sub(amount)
            .expect_or_reject("Invariant violation: aggregate storage balance underflow");
        let balance = self
            .withdraw_from_storage_account(&account_id, amount)
            .unwrap_or_else(|error| env::panic_str(&error.to_string()));
        self.available_storage_balance = aggregate;
        self.assert_storage_solvency(amount);
        if !amount.is_zero() {
            Promise::new(account_id).transfer(amount).detach();
        }
        balance
    }

    #[payable]
    pub fn storage_unregister(&mut self, force: Option<bool>) -> bool {
        assert_one_yocto();
        let account_id = env::predecessor_account_id();
        let Ok(balance) = self.get_storage_balance(&account_id) else {
            return false;
        };
        if force.unwrap_or(false) {
            // The hook deliberately rejects even accounts without retained code.
            self.force_unregister_storage_account(&account_id)
                .unwrap_or_else(|error| env::panic_str(&error.to_string()));
            unreachable!("Force unregistration hook must reject")
        }
        let minimum = self.get_storage_balance_bounds().min;
        let locked = balance
            .total
            .checked_sub(balance.available)
            .expect_or_reject("Invariant violation: available storage exceeds total");
        require!(
            locked == minimum,
            "Cannot unregister with locked storage balance"
        );
        let aggregate = self
            .available_storage_balance
            .checked_sub(balance.available)
            .expect_or_reject("Invariant violation: aggregate storage balance underflow");
        // The controller requires *zero* locked balance to unregister. Release
        // only the verified registration reserve immediately before removing it.
        self.unlock_storage(&account_id, minimum)
            .unwrap_or_else(|error| env::panic_str(&error.to_string()));
        let refund = self
            .unregister_storage_account(&account_id)
            .unwrap_or_else(|error| env::panic_str(&error.to_string()));
        self.available_storage_balance = aggregate;
        self.assert_storage_solvency(refund);
        Promise::new(account_id).transfer(refund).detach();
        true
    }

    pub fn storage_balance_of(&self, account_id: AccountId) -> Option<StorageBalance> {
        self.get_storage_balance(&account_id).ok()
    }

    pub fn storage_balance_bounds(&self) -> StorageBalanceBounds {
        self.get_storage_balance_bounds()
    }
}

#[cfg(test)]
mod tests {
    use near_sdk::{
        mock::MockAction,
        test_utils::{get_created_receipts, VMContextBuilder},
        testing_env,
    };
    use rstest::rstest;

    use super::*;

    fn account(name: &str) -> AccountId {
        name.parse().unwrap()
    }

    fn tokens(amount: u128) -> NearToken {
        NearToken::from_yoctonear(amount)
    }

    fn context(predecessor: &str, deposit: NearToken) {
        testing_env!(VMContextBuilder::new()
            .predecessor_account_id(account(predecessor))
            .attached_deposit(deposit)
            .account_balance(NearToken::from_near(100))
            .storage_usage(env::storage_usage())
            .build());
    }

    fn setup() -> Contract {
        testing_env!(VMContextBuilder::new()
            .predecessor_account_id(account("owner.near"))
            .account_balance(NearToken::from_near(100))
            .build());
        Contract::new()
    }

    fn register(contract: &mut Contract, name: &str, extra: NearToken) -> StorageBalance {
        let deposit = contract
            .storage_balance_bounds()
            .min
            .checked_add(extra)
            .unwrap();
        context(name, deposit);
        contract.storage_deposit(None, None)
    }

    fn transfers() -> Vec<(AccountId, NearToken)> {
        get_created_receipts()
            .into_iter()
            .flat_map(|receipt| {
                receipt
                    .actions
                    .into_iter()
                    .filter_map(move |action| match action {
                        MockAction::Transfer { deposit, .. } => {
                            Some((receipt.receiver_id.clone(), deposit))
                        }
                        _ => None,
                    })
            })
            .collect()
    }

    fn assert_panics(expected: &str, f: impl FnOnce()) {
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_err();
        let actual = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap();
        assert!(
            actual.contains(expected),
            "expected {expected:?}, got {actual:?}"
        );
    }

    #[test]
    fn minimum_exactly_covers_fixed_size_registration() {
        let mut contract = setup();
        let bounds = contract.storage_balance_bounds();
        assert!(bounds.min > NearToken::ZERO);
        assert_eq!(bounds.max, None);
        let before = env::storage_usage();
        let balance = register(&mut contract, "alice.near", NearToken::ZERO);
        let bytes = env::storage_usage().checked_sub(before).unwrap();
        assert_eq!(
            bounds.min,
            env::storage_byte_cost()
                .checked_mul(u128::from(bytes))
                .unwrap()
        );
        assert_eq!(
            balance,
            StorageBalance {
                total: bounds.min,
                available: NearToken::ZERO
            }
        );
        assert_eq!(contract.available_storage_balance, NearToken::ZERO);
        assert!(transfers().is_empty());
        let before = env::storage_usage();
        register(
            &mut contract,
            "a-much-longer-account-name.near",
            NearToken::ZERO,
        );
        assert_eq!(env::storage_usage() - before, bytes);
    }

    #[test]
    fn third_party_registration_only_refunds_payer_and_preserves_target() {
        let mut contract = setup();
        let minimum = contract.storage_balance_bounds().min;
        context("sponsor.near", minimum.checked_add(tokens(17)).unwrap());
        let balance = contract.storage_deposit(Some(account("alice.near")), Some(true));
        assert_eq!(balance.total, minimum);
        assert_eq!(balance.available, NearToken::ZERO);
        assert_eq!(transfers(), vec![(account("sponsor.near"), tokens(17))]);
        assert_eq!(contract.storage_balance_of(account("sponsor.near")), None);
        assert_eq!(
            contract.storage_balance_of(account("alice.near")),
            Some(balance.clone())
        );
        context("sponsor.near", tokens(9));
        assert_eq!(
            contract.storage_deposit(Some(account("alice.near")), Some(true)),
            balance
        );
        assert_eq!(transfers(), vec![(account("sponsor.near"), tokens(9))]);
        assert_eq!(contract.available_storage_balance, NearToken::ZERO);
    }

    #[test]
    fn deposits_and_lock_release_track_every_accounts_available_balance() {
        let mut contract = setup();
        let alice = account("alice.near");
        let minimum = contract.storage_balance_bounds().min;
        register(&mut contract, "alice.near", tokens(100));
        register(&mut contract, "bob.near", tokens(40));
        context("sponsor.near", tokens(20));
        let balance = contract.storage_deposit(Some(alice.clone()), None);
        assert_eq!(balance.total, minimum.checked_add(tokens(120)).unwrap());
        assert_eq!(balance.available, tokens(120));
        assert_eq!(contract.available_storage_balance, tokens(160));
        contract.charge_storage(&alice, tokens(50));
        assert_eq!(
            contract
                .storage_balance_of(alice.clone())
                .unwrap()
                .available,
            tokens(70)
        );
        assert_eq!(contract.available_storage_balance, tokens(110));
        contract.release_storage(&alice, tokens(30));
        assert_eq!(
            contract
                .storage_balance_of(alice.clone())
                .unwrap()
                .available,
            tokens(100)
        );
        assert_eq!(contract.available_storage_balance, tokens(140));
        assert_panics("Cannot release registration", || {
            contract.release_storage(&alice, tokens(21));
        });
        assert_eq!(
            contract.storage_balance_of(alice).unwrap().available,
            tokens(100)
        );
        assert_eq!(contract.available_storage_balance, tokens(140));
    }

    #[test]
    fn withdrawals_support_partial_all_zero_and_reject_excess() {
        let mut contract = setup();
        let minimum = contract.storage_balance_bounds().min;
        register(&mut contract, "alice.near", tokens(100));
        context("alice.near", tokens(1));
        let balance = contract.storage_withdraw(Some(tokens(30)));
        assert_eq!(balance.total, minimum.checked_add(tokens(70)).unwrap());
        assert_eq!(balance.available, tokens(70));
        assert_eq!(contract.available_storage_balance, tokens(70));
        assert_eq!(transfers(), vec![(account("alice.near"), tokens(30))]);
        context("alice.near", tokens(1));
        assert_panics("Insufficient available", || {
            contract.storage_withdraw(Some(tokens(71)));
        });
        assert_eq!(contract.available_storage_balance, tokens(70));
        context("alice.near", tokens(1));
        assert_eq!(
            contract.storage_withdraw(None),
            StorageBalance {
                total: minimum,
                available: NearToken::ZERO
            }
        );
        assert_eq!(transfers(), vec![(account("alice.near"), tokens(70))]);
        assert_eq!(contract.available_storage_balance, NearToken::ZERO);
        context("alice.near", tokens(1));
        assert_eq!(contract.storage_withdraw(None).available, NearToken::ZERO);
        assert!(transfers().is_empty());
    }

    #[test]
    fn unregister_requires_release_and_refunds_total_without_other_users_credit() {
        let mut contract = setup();
        let alice = account("alice.near");
        let minimum = contract.storage_balance_bounds().min;
        register(&mut contract, "alice.near", tokens(100));
        register(&mut contract, "bob.near", tokens(20));
        contract.charge_storage(&alice, tokens(60));
        context("alice.near", tokens(1));
        assert_panics("Cannot unregister with locked", || {
            contract.storage_unregister(None);
        });
        assert_eq!(contract.available_storage_balance, tokens(60));
        assert!(contract.storage_balance_of(alice.clone()).is_some());
        contract.release_storage(&alice, tokens(60));
        context("alice.near", tokens(1));
        assert!(contract.storage_unregister(Some(false)));
        assert_eq!(contract.storage_balance_of(alice), None);
        assert_eq!(contract.available_storage_balance, tokens(20));
        assert_eq!(
            transfers(),
            vec![(
                account("alice.near"),
                minimum.checked_add(tokens(100)).unwrap()
            )]
        );
        context("alice.near", tokens(1));
        assert!(!contract.storage_unregister(None));
        assert!(transfers().is_empty());
    }

    #[test]
    fn unregister_minimum_only_releases_registration_reserve() {
        let mut contract = setup();
        let minimum = contract.storage_balance_bounds().min;
        register(&mut contract, "alice.near", NearToken::ZERO);
        context("alice.near", tokens(1));
        assert!(contract.storage_unregister(None));
        assert_eq!(contract.available_storage_balance, NearToken::ZERO);
        assert_eq!(transfers(), vec![(account("alice.near"), minimum)]);
    }

    #[test]
    fn force_rejects_registered_accounts_and_does_not_erase_their_balance() {
        let mut contract = setup();
        let balance = register(&mut contract, "alice.near", tokens(100));
        context("alice.near", tokens(1));
        assert_panics("Forced storage unregistration is not supported", || {
            contract.storage_unregister(Some(true));
        });
        assert_eq!(
            contract.storage_balance_of(account("alice.near")),
            Some(balance)
        );
        assert_eq!(contract.available_storage_balance, tokens(100));
        context("absent.near", tokens(1));
        assert!(!contract.storage_unregister(Some(true)));
        assert!(transfers().is_empty());
    }

    #[test]
    fn registration_underpayment_and_unregistered_operations_fail() {
        let mut contract = setup();
        let minimum = contract.storage_balance_bounds().min;
        context("alice.near", minimum.checked_sub(tokens(1)).unwrap());
        assert_panics("Insufficient storage deposit", || {
            contract.storage_deposit(None, None);
        });
        assert_eq!(contract.storage_balance_of(account("alice.near")), None);
        assert_eq!(contract.available_storage_balance, NearToken::ZERO);
        context("alice.near", tokens(1));
        assert_panics("is not registered", || {
            contract.storage_withdraw(None);
        });
        assert_panics("is not registered", || {
            contract.release_storage(&account("alice.near"), tokens(1));
        });
    }

    #[test]
    fn charge_rejects_unregistered_or_underfunded_payers_without_changing_ledger() {
        let mut contract = setup();
        let alice = account("alice.near");
        assert_panics("is not registered", || {
            contract.charge_storage(&alice, tokens(1));
        });
        assert_eq!(contract.available_storage_balance, NearToken::ZERO);
        let balance = register(&mut contract, "alice.near", tokens(10));
        assert_panics("insufficient balance", || {
            contract.charge_storage(&alice, tokens(11));
        });
        assert_eq!(
            contract.storage_balance_of(alice.clone()),
            Some(balance.clone())
        );
        assert_eq!(contract.available_storage_balance, tokens(10));
        contract.charge_storage(&alice, NearToken::ZERO);
        contract.release_storage(&alice, NearToken::ZERO);
        assert_eq!(contract.storage_balance_of(alice), Some(balance));
        assert_eq!(contract.available_storage_balance, tokens(10));
    }

    #[test]
    fn storage_views_keep_standard_json_amounts_and_absence() {
        let mut contract = setup();
        assert_eq!(contract.storage_balance_of(account("absent.near")), None);
        let bounds = contract.storage_balance_bounds();
        assert_eq!(
            near_sdk::serde_json::to_value(&bounds).unwrap(),
            near_sdk::serde_json::json!({ "min": bounds.min.as_yoctonear().to_string(), "max": null }),
        );
        let balance = register(&mut contract, "alice.near", tokens(10));
        assert_eq!(
            near_sdk::serde_json::to_value(balance).unwrap(),
            near_sdk::serde_json::json!({
                "total": bounds.min.checked_add(tokens(10)).unwrap().as_yoctonear().to_string(),
                "available": "10",
            }),
        );
    }

    #[rstest]
    fn withdrawal_and_unregistration_require_one_yocto(
        #[values(0, 2)] deposit: u128,
        #[values(false, true)] unregister: bool,
    ) {
        let mut contract = setup();
        register(&mut contract, "alice.near", tokens(100));
        context("alice.near", tokens(deposit));
        assert_panics("Requires attached deposit of exactly 1 yoctoNEAR", || {
            if unregister {
                contract.storage_unregister(None);
            } else {
                contract.storage_withdraw(None);
            }
        });
        assert_eq!(contract.available_storage_balance, tokens(100));
    }

    #[test]
    fn solvency_protects_storage_and_unwithdrawn_credits() {
        let mut contract = setup();
        register(&mut contract, "alice.near", tokens(100));
        let required = env::storage_byte_cost()
            .checked_mul(u128::from(env::storage_usage()))
            .unwrap()
            .checked_add(tokens(100))
            .unwrap()
            .checked_add(contract.owner_storage_reserve())
            .unwrap();
        testing_env!(VMContextBuilder::new()
            .account_balance(required.checked_add(tokens(10)).unwrap())
            .storage_usage(env::storage_usage())
            .build());
        contract.assert_storage_solvency(tokens(10));
        assert_panics(
            "Insufficient registry balance to cover storage liabilities",
            || {
                contract.assert_storage_solvency(tokens(11));
            },
        );
    }
}
