use near_sdk::near;

use crate::asset::BorrowAssetAmount;

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh])]
pub struct VirtualCredit {
    /// Amount of virtual credit that has been paid by borrowers, but the
    /// corresponding amount of virtual supply has not been created by
    /// suppliers yet.
    excess_virtual_credit: BorrowAssetAmount,
    /// Amount of virtuals supply that has been created by suppliers, but the
    /// corresponding amount of fees/interest has not yet been paid by
    /// borrowers.
    uncredited_virtual_supply: BorrowAssetAmount,
    entries: Vec<Entry>,
}

impl VirtualCredit {
    #[cfg(not(test))]
    #[inline(always)]
    #[allow(clippy::unused_self)]
    fn check_invariants(&self) {}
    #[cfg(test)]
    fn check_invariants(&self) {
        debug_assert!(
            self.excess_virtual_credit.is_zero() || self.uncredited_virtual_supply.is_zero(),
            "both excess credit and uncredited supply may not be nonzero simultaneously"
        );

        let mut virtual_supply_total = BorrowAssetAmount::zero();
        let mut virtual_credit_total = BorrowAssetAmount::zero();
        for entry in &self.entries {
            virtual_supply_total += entry.add_market_virtual;
            virtual_credit_total += entry.add_virtual_credit;

            debug_assert!(
                virtual_supply_total >= virtual_credit_total,
                "entries should never have more credit than supply"
            );
        }

        for i in 1..self.entries.len() {
            let prev = &self.entries[i - 1];
            let curr = &self.entries[i];

            debug_assert!(
                prev.snapshot_index < curr.snapshot_index,
                "snapshot indices did not increase: {} >= {}",
                prev.snapshot_index,
                curr.snapshot_index,
            );
        }
    }

    fn entry_index_for_snapshot_index(&self, snapshot_index: u32) -> usize {
        match self
            .entries
            .binary_search_by(|e| e.snapshot_index.cmp(&snapshot_index))
        {
            Ok(i) | Err(i) => i,
        }
    }

    fn reduce(&mut self, from_entry_index: usize, market_virtual_amount: BorrowAssetAmount) {
        let mut remaining_reduction = market_virtual_amount;

        for entry in &mut self.entries[from_entry_index..] {
            if entry.add_market_virtual >= remaining_reduction {
                entry.add_market_virtual -= remaining_reduction;
                remaining_reduction = 0.into();
                break;
            }

            remaining_reduction -= entry.add_market_virtual;
            entry.add_market_virtual = 0.into();
        }

        if !remaining_reduction.is_zero() {
            crate::panic_with_message(
                "Invariant violation: attempted to remove more market virtual than available",
            );
        }
    }

    fn merge(&mut self, from_entry_index: usize) {
        let mut merged = Vec::new();
        let mut prev = Entry::new(0, 0, 0);
        for current in &self.entries[from_entry_index..] {
            if (current.add_market_virtual.is_zero() && prev.add_market_virtual.is_zero())
                || prev.add_virtual_credit.is_zero()
            {
                prev.snapshot_index = current.snapshot_index;
                prev.add_market_virtual += current.add_market_virtual;
                prev.add_virtual_credit += current.add_virtual_credit;
            } else {
                merged.push(prev);
                prev = current.clone();
            }
        }
        merged.push(prev);

        self.entries.splice(from_entry_index.., merged);
    }

    pub fn add_virtual_supply(&mut self, snapshot_index: u32, amount: BorrowAssetAmount) {
        self.check_invariants();

        eprintln!("add_virtual_supply({snapshot_index}, {amount})");
        let realized_excess = if amount > self.excess_virtual_credit {
            let realized_excess = self.excess_virtual_credit;
            self.uncredited_virtual_supply += amount - realized_excess;
            self.excess_virtual_credit = 0.into();
            realized_excess
        } else {
            self.excess_virtual_credit -= amount;
            amount
        };

        eprintln!("add_virtual_supply[realized_excess = {realized_excess}]");

        if let Some(current) = self
            .entries
            .last_mut()
            .filter(|d| d.snapshot_index == snapshot_index || d.add_virtual_credit.is_zero())
        {
            eprintln!("add_virtual_supply[current = {current:?}]");
            current.snapshot_index = snapshot_index;
            current.add_market_virtual += amount;
            current.add_virtual_credit += realized_excess;
        } else {
            eprintln!("add_virtual_supply[pushing({snapshot_index}, {realized_excess}, {amount})]");
            self.entries
                .push(Entry::new(snapshot_index, realized_excess, amount));
        }

        self.check_invariants();
    }

    pub fn add_virtual_credit(&mut self, amount: BorrowAssetAmount) {
        self.check_invariants();

        eprintln!("add_virtual_credit({amount})");
        let amount_to_insert = if amount > self.uncredited_virtual_supply {
            let uncredited = self.uncredited_virtual_supply;
            self.excess_virtual_credit += amount - uncredited;
            self.uncredited_virtual_supply = 0.into();
            uncredited
        } else {
            self.uncredited_virtual_supply -= amount;
            amount
        };
        eprintln!("add_virtual_credit[amount_to_insert = {amount_to_insert}]");

        if !amount_to_insert.is_zero() {
            if let Some(current) = self.entries.last_mut() {
                current.add_virtual_credit += amount_to_insert;
            } else {
                crate::panic_with_message(
                    "Invariant violation: entries must not be empty if amount_to_insert != 0",
                );
            }
        }

        self.check_invariants();
    }

    pub fn realize(
        &mut self,
        from_snapshot_index: u32,
        until_snapshot_index: u32,
        mut position_virtual: BorrowAssetAmount,
    ) -> Realization {
        self.check_invariants();

        if from_snapshot_index > until_snapshot_index {
            crate::panic_with_message(&format!("Invariant violation: from_snapshot_index > until_snapshot_index ({from_snapshot_index} > {until_snapshot_index})"));
        }

        let mut market_virtual = BorrowAssetAmount::zero();
        let mut realized = BorrowAssetAmount::zero();
        let mut last_snapshot_index = from_snapshot_index;

        for entry in &mut self.entries {
            if entry.snapshot_index >= until_snapshot_index {
                break;
            }

            market_virtual += entry.add_market_virtual;

            if entry.snapshot_index >= from_snapshot_index {
                let r = entry
                    .add_virtual_credit
                    .min(market_virtual)
                    .min(position_virtual);

                position_virtual -= r;
                entry.add_virtual_credit -= r;
                market_virtual -= r;
                realized += r;

                last_snapshot_index = entry.snapshot_index;
            }
        }

        let from_entry_index = self.entry_index_for_snapshot_index(from_snapshot_index);
        self.reduce(from_entry_index, realized);
        self.merge(from_entry_index);

        self.check_invariants();

        Realization {
            amount: realized,
            until_snapshot_index: last_snapshot_index + 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Realization {
    pub amount: BorrowAssetAmount,
    pub until_snapshot_index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh])]
struct Entry {
    snapshot_index: u32,
    add_virtual_credit: BorrowAssetAmount,
    add_market_virtual: BorrowAssetAmount,
}

impl Entry {
    pub fn new(
        snapshot_index: u32,
        add_virtual_credit: impl Into<BorrowAssetAmount>,
        add_market_virtual: impl Into<BorrowAssetAmount>,
    ) -> Self {
        Self {
            snapshot_index,
            add_virtual_credit: add_virtual_credit.into(),
            add_market_virtual: add_market_virtual.into(),
        }
    }
}

#[allow(clippy::cast_possible_truncation)]
#[cfg(test)]
mod tests {
    use super::*;

    enum Cmd {
        VirtualSupply(u128),
        Credit(u128),
    }

    #[rstest::rstest]
    fn basic() {
        let mut v = VirtualCredit::default();

        v.add_virtual_supply(0, 0.into());

        assert_eq!(v.entries, vec![Entry::new(0, 0, 0)]);

        v.add_virtual_supply(0, 0.into());

        assert_eq!(
            v.entries,
            vec![Entry::new(0, 0, 0)],
            "merge identical zero entries"
        );

        v.add_virtual_supply(3, 0.into());
        v.add_virtual_supply(5, 0.into());

        assert_eq!(
            v.entries,
            vec![Entry::new(5, 0, 0)],
            "merge zero entries to highest snapshot"
        );

        v.add_virtual_supply(6, 60.into());
        v.add_virtual_supply(7, 70.into());

        assert_eq!(
            v.entries,
            vec![Entry::new(7, 0, 130)],
            "merge entries with zero virtual credit to highest snapshot"
        );

        v.add_virtual_credit(80.into());

        assert_eq!(
            v.entries,
            vec![Entry::new(7, 80, 130)],
            "add virtual credit < market virtual"
        );

        v.add_virtual_supply(8, 10.into());

        assert_eq!(
            v.entries,
            vec![Entry::new(7, 80, 130), Entry::new(8, 0, 10)],
            "adding more virtual supply in same snapshot is ok"
        );

        v.add_virtual_supply(9, 10.into());

        assert_eq!(
            v.entries,
            vec![Entry::new(7, 80, 130), Entry::new(9, 0, 20)],
        );

        v.add_virtual_credit(100.into());

        assert_eq!(
            v.entries,
            vec![Entry::new(7, 80, 130), Entry::new(9, 70, 20)],
        );

        v.add_virtual_supply(10, 20.into());

        assert_eq!(
            v.entries,
            vec![
                Entry::new(7, 80, 130),
                Entry::new(9, 70, 20),
                Entry::new(10, 20, 20),
            ],
        );

        v.add_virtual_supply(11, 20.into());

        assert_eq!(
            v.entries,
            vec![
                Entry::new(7, 80, 130),
                Entry::new(9, 70, 20),
                Entry::new(10, 20, 20),
                Entry::new(11, 10, 20),
            ],
        );

        let r = v.realize(0, 10, 140.into());
        assert_eq!(r.amount, 140.into());
        assert_eq!(r.until_snapshot_index, 10);

        assert_eq!(
            v.entries,
            vec![
                Entry::new(9, 10, 10),
                Entry::new(10, 20, 20),
                Entry::new(11, 10, 20),
            ],
        );

        let r = v.realize(10, 11, 10.into());
        assert_eq!(r.amount, 10.into());
        assert_eq!(r.until_snapshot_index, 11);

        assert_eq!(
            v.entries,
            vec![
                Entry::new(9, 10, 10),
                Entry::new(10, 10, 10),
                Entry::new(11, 10, 20),
            ],
        );

        let r = v.realize(0, 12, 40.into());
        assert_eq!(r.amount, 30.into());
        assert_eq!(r.until_snapshot_index, 12);

        assert_eq!(v.entries, vec![Entry::new(11, 0, 10)]);
    }

    #[rstest::rstest]
    fn multiple_positions(
        #[values(
            &[Cmd::VirtualSupply(100), Cmd::VirtualSupply(10), Cmd::Credit(50), Cmd::Credit(50), Cmd::VirtualSupply(20)],
            &[Cmd::VirtualSupply(100), Cmd::Credit(10), Cmd::Credit(50), Cmd::VirtualSupply(50), Cmd::VirtualSupply(20), Cmd::Credit(40)],
            &[Cmd::Credit(100), Cmd::Credit(10), Cmd::Credit(50), Cmd::VirtualSupply(50), Cmd::VirtualSupply(20), Cmd::VirtualSupply(30)],
        )]
        ops: &[Cmd],
        #[values(&[50, 40, 10])] positions: &[u128],
    ) {
        let mut v = VirtualCredit::default();

        for (i, op) in ops.iter().enumerate() {
            match op {
                Cmd::VirtualSupply(supply) => {
                    v.add_virtual_supply(i as u32, (*supply).into());
                }
                Cmd::Credit(credit) => {
                    v.add_virtual_credit((*credit).into());
                }
            }
        }

        for position in positions {
            let position = BorrowAssetAmount::new(*position);

            let r = v.realize(0, ops.len() as u32, position);
            assert_eq!(r.amount, position);
        }
    }

    #[rstest::rstest]
    fn single_position(
        #[values(&[0, 1, 2, 3, 4, 5], &[0, 2, 5], &[0, 5], &[1, 5])] splits: &[u32],
        #[values(1, 25, 50, 100)] position_virtual: u128,
    ) {
        let position_virtual: BorrowAssetAmount = position_virtual.into();

        let mut v = VirtualCredit::default();
        v.add_virtual_supply(1, 50.into());
        v.add_virtual_supply(2, 50.into());
        v.add_virtual_credit(10.into());
        v.add_virtual_credit(100.into());
        v.add_virtual_supply(4, 50.into());
        let mut total = BorrowAssetAmount::zero();
        let mut remaining = position_virtual;
        for i in 1..splits.len() {
            let a = splits[i - 1];
            let b = splits[i];
            let r = v.realize(a, b, remaining);
            remaining -= r.amount;
            total += r.amount;
        }
        assert_eq!(total, position_virtual);
    }
}
