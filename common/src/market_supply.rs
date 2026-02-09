use near_sdk::near;

use crate::asset::BorrowAssetAmount;

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh])]
pub struct MarketSupply {
    amount_virtual: BorrowAssetAmount,
    amount_real: BorrowAssetAmount,
    amount_realizable_through_current_snapshot: BorrowAssetAmount,
    /// Amount of virtual credit that has been paid by borrowers, but the
    /// corresponding amount of virtual supply has not been created by
    /// suppliers yet.
    excess_virtual_credit: BorrowAssetAmount,
    /// Amount of virtuals supply that has been created by suppliers, but the
    /// corresponding amount of fees/interest has not yet been paid by
    /// borrowers.
    uncredited_virtual_supply: BorrowAssetAmount,
    virtual_entries: Vec<VirtualEntry>,
}

pub struct SupplySplit {
    pub real: BorrowAssetAmount,
    pub r#virtual: BorrowAssetAmount,
}

impl MarketSupply {
    #[cfg(not(test))]
    #[inline(always)]
    #[allow(clippy::unused_self)]
    fn check_invariants(&self) {}
    #[cfg(test)]
    fn check_invariants(&self) {
        assert_eq!(
            self.virtual_entries
                .iter()
                .map(|e| e.add_market_virtual)
                .sum::<BorrowAssetAmount>(),
            self.amount_virtual,
            "total virtual should equal sum of virtual entries",
        );

        debug_assert!(
            self.excess_virtual_credit.is_zero() || self.uncredited_virtual_supply.is_zero(),
            "both excess credit and uncredited supply may not be nonzero simultaneously"
        );

        let mut virtual_supply_total = BorrowAssetAmount::zero();
        let mut virtual_credit_total = BorrowAssetAmount::zero();
        for entry in &self.virtual_entries {
            virtual_supply_total += entry.add_market_virtual;
            virtual_credit_total += entry.add_virtual_credit;

            debug_assert!(
                virtual_supply_total >= virtual_credit_total,
                "entries should never have more credit than supply"
            );
        }

        for i in 1..self.virtual_entries.len() {
            let prev = &self.virtual_entries[i - 1];
            let curr = &self.virtual_entries[i];

            debug_assert!(
                prev.snapshot_index < curr.snapshot_index,
                "snapshot indices did not increase: {} >= {}",
                prev.snapshot_index,
                curr.snapshot_index,
            );
        }
    }

    pub fn total(&self) -> BorrowAssetAmount {
        self.amount_real + self.amount_virtual
    }

    pub fn real(&self) -> BorrowAssetAmount {
        self.amount_real + self.amount_realizable_through_current_snapshot
    }

    pub fn r#virtual(&self) -> BorrowAssetAmount {
        self.amount_virtual - self.amount_realizable_through_current_snapshot
    }

    pub fn add_virtual_credit(&mut self, snapshot_index: u32, amount: BorrowAssetAmount) {
        self.check_invariants();

        let amount_to_insert = if amount > self.uncredited_virtual_supply {
            let uncredited = self.uncredited_virtual_supply;
            self.excess_virtual_credit += amount - uncredited;
            self.uncredited_virtual_supply = 0.into();
            uncredited
        } else {
            self.uncredited_virtual_supply -= amount;
            amount
        };

        if !amount_to_insert.is_zero() {
            if let Some(current) = self
                .virtual_entries
                .last_mut()
                .filter(|e| e.snapshot_index == snapshot_index)
            {
                current.add_virtual_credit += amount_to_insert;
            } else {
                self.virtual_entries
                    .push(VirtualEntry::new(snapshot_index, amount_to_insert, 0));
            }
            self.amount_realizable_through_current_snapshot += amount_to_insert;
        }

        self.check_invariants();
    }

    /// May be called a maximum of one time for each `snapshot_index` value.
    /// A later call to this function must have `snapshot_index` greater than all previous calls.
    pub fn advance(
        &mut self,
        snapshot_index: u32,
        amount_real: BorrowAssetAmount,
        amount_virtual: BorrowAssetAmount,
    ) {
        self.check_invariants();

        // Real
        self.amount_real += amount_real;

        // Virtual
        let take_excess = if amount_virtual > self.excess_virtual_credit {
            let take_excess = self.excess_virtual_credit;
            self.uncredited_virtual_supply += amount_virtual - take_excess;
            self.excess_virtual_credit = 0.into();
            take_excess
        } else {
            self.excess_virtual_credit -= amount_virtual;
            amount_virtual
        };
        self.amount_realizable_through_current_snapshot += take_excess;

        if !(take_excess.is_zero() && amount_virtual.is_zero()) {
            self.virtual_entries.push(VirtualEntry::new(
                snapshot_index,
                take_excess,
                amount_virtual,
            ));
        }

        self.amount_virtual += amount_virtual;

        self.check_invariants();
    }

    pub fn remove_real(&mut self, amount: BorrowAssetAmount) {
        self.amount_real -= amount;
    }

    pub fn realize(
        &mut self,
        from_snapshot_index: u32,
        until_snapshot_index: u32,
        mut position_virtual: BorrowAssetAmount,
    ) -> Realization {
        self.check_invariants();
        eprintln!("from: {from_snapshot_index}");
        eprintln!("until: {until_snapshot_index}");

        if from_snapshot_index > until_snapshot_index {
            crate::panic_with_message(&format!("Invariant violation: from_snapshot_index > until_snapshot_index ({from_snapshot_index} > {until_snapshot_index})"));
        }

        let mut market_virtual = BorrowAssetAmount::zero();
        let mut realized = BorrowAssetAmount::zero();
        let mut looped_until = from_snapshot_index;
        let mut reduce_from_entry_index = None;

        for (i, entry) in self.virtual_entries.iter_mut().enumerate() {
            eprintln!("entry: {}", entry.snapshot_index);
            if entry.snapshot_index >= until_snapshot_index {
                break;
            }

            market_virtual += entry.add_market_virtual;

            if entry.snapshot_index >= from_snapshot_index {
                let r = entry
                    .add_virtual_credit
                    .min(market_virtual)
                    .min(position_virtual);
                eprintln!("r: {r}");

                position_virtual -= r;
                entry.add_virtual_credit -= r;
                market_virtual -= r;
                realized += r;

                reduce_from_entry_index = reduce_from_entry_index.or(Some(i));
                looped_until = entry.snapshot_index + 1;
            }
        }
        self.amount_virtual -= realized;

        // Reduce market virtual to reflect the amount realized.
        //
        // This needs to be a separate loop because we do not know the value
        // of `realized` until the first loop has completed.
        let mut remaining_reduction = realized;
        if let Some(reduce_from_entry_index) = reduce_from_entry_index {
            for entry in &mut self.virtual_entries[reduce_from_entry_index..] {
                if entry.add_market_virtual >= remaining_reduction {
                    entry.add_market_virtual -= remaining_reduction;
                    remaining_reduction = 0.into();
                    break;
                }

                remaining_reduction -= entry.add_market_virtual;
                entry.add_market_virtual = 0.into();
            }

            self.virtual_entries
                .retain(|e| !e.add_market_virtual.is_zero() || !e.add_virtual_credit.is_zero());
        }

        if !remaining_reduction.is_zero() {
            crate::panic_with_message(
                "Invariant violation: attempted to remove more market virtual than available",
            );
        }

        self.amount_realizable_through_current_snapshot -= realized;

        self.check_invariants();

        Realization {
            amount: realized,
            until_snapshot_index: looped_until,
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
struct VirtualEntry {
    snapshot_index: u32,
    add_virtual_credit: BorrowAssetAmount,
    add_market_virtual: BorrowAssetAmount,
}

impl VirtualEntry {
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
        let mut v = MarketSupply::default();

        v.advance(0, 0.into(), 0.into());

        assert_eq!(v.virtual_entries, vec![]);
        assert_eq!(v.excess_virtual_credit, 0.into());
        assert_eq!(v.uncredited_virtual_supply, 0.into());

        v.advance(1, 0.into(), 0.into());

        assert_eq!(v.virtual_entries, vec![]);
        assert_eq!(v.excess_virtual_credit, 0.into());
        assert_eq!(v.uncredited_virtual_supply, 0.into());

        v.advance(6, 0.into(), 60.into());
        v.advance(7, 0.into(), 70.into());

        assert_eq!(
            v.virtual_entries,
            vec![VirtualEntry::new(6, 0, 60), VirtualEntry::new(7, 0, 70)],
        );
        assert_eq!(v.excess_virtual_credit, 0.into());
        assert_eq!(v.uncredited_virtual_supply, 130.into());

        v.advance(8, 0.into(), 10.into());

        assert_eq!(
            v.virtual_entries,
            vec![
                VirtualEntry::new(6, 0, 60),
                VirtualEntry::new(7, 0, 70),
                VirtualEntry::new(8, 0, 10),
            ],
        );
        assert_eq!(v.excess_virtual_credit, 0.into());
        assert_eq!(v.uncredited_virtual_supply, 140.into());

        v.advance(9, 0.into(), 10.into());

        assert_eq!(
            v.virtual_entries,
            vec![
                VirtualEntry::new(6, 0, 60),
                VirtualEntry::new(7, 0, 70),
                VirtualEntry::new(8, 0, 10),
                VirtualEntry::new(9, 0, 10),
            ],
        );
        assert_eq!(v.excess_virtual_credit, 0.into());
        assert_eq!(v.uncredited_virtual_supply, 150.into());

        v.add_virtual_credit(9, 180.into());

        assert_eq!(
            v.virtual_entries,
            vec![
                VirtualEntry::new(6, 0, 60),
                VirtualEntry::new(7, 0, 70),
                VirtualEntry::new(8, 0, 10),
                VirtualEntry::new(9, 150, 10),
            ],
        );
        assert_eq!(v.excess_virtual_credit, 30.into());
        assert_eq!(v.uncredited_virtual_supply, 0.into());

        v.advance(10, 0.into(), 20.into());

        assert_eq!(
            v.virtual_entries,
            vec![
                VirtualEntry::new(6, 0, 60),
                VirtualEntry::new(7, 0, 70),
                VirtualEntry::new(8, 0, 10),
                VirtualEntry::new(9, 150, 10),
                VirtualEntry::new(10, 20, 20),
            ],
        );
        assert_eq!(v.excess_virtual_credit, 10.into());
        assert_eq!(v.uncredited_virtual_supply, 0.into());

        v.advance(11, 0.into(), 20.into());

        assert_eq!(
            v.virtual_entries,
            vec![
                VirtualEntry::new(6, 0, 60),
                VirtualEntry::new(7, 0, 70),
                VirtualEntry::new(8, 0, 10),
                VirtualEntry::new(9, 150, 10),
                VirtualEntry::new(10, 20, 20),
                VirtualEntry::new(11, 10, 20),
            ],
        );
        assert_eq!(v.excess_virtual_credit, 0.into());
        assert_eq!(v.uncredited_virtual_supply, 10.into());

        let r = v.realize(0, 10, 140.into());
        assert_eq!(r.amount, 140.into());
        assert_eq!(r.until_snapshot_index, 10);

        assert_eq!(
            v.virtual_entries,
            vec![
                VirtualEntry::new(9, 10, 10),
                VirtualEntry::new(10, 20, 20),
                VirtualEntry::new(11, 10, 20),
            ],
        );
        assert_eq!(v.excess_virtual_credit, 0.into());
        assert_eq!(v.uncredited_virtual_supply, 10.into());

        let r = v.realize(10, 11, 10.into());
        assert_eq!(r.amount, 10.into());
        assert_eq!(r.until_snapshot_index, 11);

        assert_eq!(
            v.virtual_entries,
            vec![
                VirtualEntry::new(9, 10, 10),
                VirtualEntry::new(10, 10, 10),
                VirtualEntry::new(11, 10, 20),
            ],
        );
        assert_eq!(v.excess_virtual_credit, 0.into());
        assert_eq!(v.uncredited_virtual_supply, 10.into());

        let r = v.realize(0, 12, 40.into());
        assert_eq!(r.amount, 30.into());
        assert_eq!(r.until_snapshot_index, 12);

        assert_eq!(v.virtual_entries, vec![VirtualEntry::new(11, 0, 10)]);
        assert_eq!(v.excess_virtual_credit, 0.into());
        assert_eq!(v.uncredited_virtual_supply, 10.into());

        v.add_virtual_credit(12, 20.into());

        assert_eq!(
            v.virtual_entries,
            vec![VirtualEntry::new(11, 0, 10), VirtualEntry::new(12, 10, 0)]
        );
        assert_eq!(v.excess_virtual_credit, 10.into());
        assert_eq!(v.uncredited_virtual_supply, 0.into());
    }

    #[rstest::rstest]
    fn multiple_positions(
        #[values(
            &[Cmd::Credit(100), Cmd::VirtualSupply(100)],
            &[Cmd::VirtualSupply(100), Cmd::Credit(100)],
            &[Cmd::Credit(10), Cmd::Credit(10), Cmd::Credit(10), Cmd::Credit(10), Cmd::Credit(10), Cmd::Credit(10), Cmd::Credit(10), Cmd::Credit(10), Cmd::Credit(10), Cmd::Credit(10), Cmd::VirtualSupply(10), Cmd::VirtualSupply(10), Cmd::VirtualSupply(10), Cmd::VirtualSupply(10), Cmd::VirtualSupply(10), Cmd::VirtualSupply(10), Cmd::VirtualSupply(10), Cmd::VirtualSupply(10), Cmd::VirtualSupply(10), Cmd::VirtualSupply(10)],
            &[Cmd::VirtualSupply(10), Cmd::VirtualSupply(10), Cmd::VirtualSupply(10), Cmd::VirtualSupply(10), Cmd::VirtualSupply(10), Cmd::VirtualSupply(10), Cmd::VirtualSupply(10), Cmd::VirtualSupply(10), Cmd::VirtualSupply(10), Cmd::VirtualSupply(10), Cmd::Credit(10), Cmd::Credit(10), Cmd::Credit(10), Cmd::Credit(10), Cmd::Credit(10), Cmd::Credit(10), Cmd::Credit(10), Cmd::Credit(10), Cmd::Credit(10), Cmd::Credit(10)],
            &[Cmd::Credit(10), Cmd::VirtualSupply(10), Cmd::Credit(10), Cmd::VirtualSupply(10), Cmd::Credit(10), Cmd::VirtualSupply(10), Cmd::Credit(10), Cmd::VirtualSupply(10), Cmd::Credit(10), Cmd::VirtualSupply(10), Cmd::Credit(10), Cmd::VirtualSupply(10), Cmd::Credit(10), Cmd::VirtualSupply(10), Cmd::Credit(10), Cmd::VirtualSupply(10), Cmd::Credit(10), Cmd::VirtualSupply(10), Cmd::Credit(10), Cmd::VirtualSupply(10)],
            &[Cmd::VirtualSupply(10), Cmd::Credit(10), Cmd::VirtualSupply(10), Cmd::Credit(10), Cmd::VirtualSupply(10), Cmd::Credit(10), Cmd::VirtualSupply(10), Cmd::Credit(10), Cmd::VirtualSupply(10), Cmd::Credit(10), Cmd::VirtualSupply(10), Cmd::Credit(10), Cmd::VirtualSupply(10), Cmd::Credit(10), Cmd::VirtualSupply(10), Cmd::Credit(10), Cmd::VirtualSupply(10), Cmd::Credit(10), Cmd::VirtualSupply(10), Cmd::Credit(10)],
        )]
        ops: &[Cmd],
        #[values(
            &[40, 30, 20, 10],
            &[10, 20, 30, 40],
            &[1; 100],
            &[100; 1],
        )]
        positions: &[u128],
    ) {
        let mut v = MarketSupply::default();

        for (i, op) in ops.iter().enumerate() {
            match op {
                Cmd::VirtualSupply(supply) => {
                    v.advance(i as u32, 0.into(), (*supply).into());
                }
                Cmd::Credit(credit) => {
                    v.add_virtual_credit(i as u32, (*credit).into());
                }
            }
        }

        for position in positions {
            let position = BorrowAssetAmount::new(*position);

            let r = v.realize(0, ops.len() as u32, position);
            assert_eq!(r.amount, position);
        }

        assert_eq!(v.virtual_entries.len(), 0);
        assert_eq!(v.excess_virtual_credit, 0.into());
        assert_eq!(v.uncredited_virtual_supply, 0.into());
    }

    #[rstest::rstest]
    fn single_position(
        #[values(&[0, 1, 2, 3, 4, 5], &[0, 2, 5], &[0, 5], &[1, 5])] splits: &[u32],
        #[values(1, 25, 50, 100)] position_virtual: u128,
    ) {
        let position_virtual: BorrowAssetAmount = position_virtual.into();

        let mut v = MarketSupply::default();
        v.advance(1, 0.into(), 50.into());
        v.advance(2, 0.into(), 50.into());
        v.add_virtual_credit(3, 10.into());
        v.add_virtual_credit(3, 100.into());
        v.advance(4, 0.into(), 50.into());
        let mut total = BorrowAssetAmount::zero();
        let mut remaining = position_virtual;
        let mut prev = splits[0];
        for i in 1..splits.len() {
            // let a = splits[i - 1];
            let b = splits[i];
            let r = v.realize(prev, b, remaining);
            eprintln!("{prev}<>{b} {r:#?}");
            prev = r.until_snapshot_index;
            remaining -= r.amount;
            total += r.amount;
        }
        eprintln!("{v:#?}");
        assert_eq!(total, position_virtual);
    }
}
