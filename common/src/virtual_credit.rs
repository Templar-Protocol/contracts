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
    fn check_invariants(&self) -> bool {
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
                curr.snapshot_index
            );
        }

        true
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
        let mut prev = Entry {
            snapshot_index: 0,
            add_virtual_credit: 0.into(),
            add_market_virtual: 0.into(),
        };
        for current in &self.entries[from_entry_index..] {
            if (current.add_market_virtual.is_zero() && prev.add_market_virtual.is_zero())
                || current.add_virtual_credit.is_zero()
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
        debug_assert!(self.check_invariants());
        let realized_excess = if amount > self.excess_virtual_credit {
            let realized_excess = self.excess_virtual_credit;
            self.uncredited_virtual_supply += amount - realized_excess;
            self.excess_virtual_credit = 0.into();
            realized_excess
        } else {
            self.excess_virtual_credit -= amount;
            amount
        };

        if !realized_excess.is_zero() {
            if let Some(current) = self
                .entries
                .last_mut()
                .filter(|d| d.snapshot_index == snapshot_index)
            {
                current.add_market_virtual += amount;
                current.add_virtual_credit += realized_excess;
            } else {
                self.entries.push(Entry {
                    snapshot_index,
                    add_virtual_credit: realized_excess,
                    add_market_virtual: amount,
                });
            }

            self.merge(self.entries.len().saturating_sub(2));
        }
        debug_assert!(self.check_invariants());
    }

    pub fn add_virtual_credit(&mut self, snapshot_index: u32, amount: BorrowAssetAmount) {
        debug_assert!(self.check_invariants());
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
                .entries
                .last_mut()
                .filter(|d| d.snapshot_index == snapshot_index)
            {
                current.add_virtual_credit += amount_to_insert;
            } else {
                self.entries.push(Entry {
                    snapshot_index,
                    add_virtual_credit: amount_to_insert,
                    add_market_virtual: 0.into(),
                });
            }

            self.merge(self.entries.len().saturating_sub(2));
        }
        debug_assert!(self.check_invariants());
    }

    pub fn realize(
        &mut self,
        from_snapshot_index: u32,
        until_snapshot_index: u32,
        mut position_virtual: BorrowAssetAmount,
    ) -> Realization {
        debug_assert!(self.check_invariants());
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

        debug_assert!(self.check_invariants());

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

        assert_eq!(
            v.entries,
            vec![Entry {
                snapshot_index: 0,
                add_virtual_credit: 0.into(),
                add_market_virtual: 0.into(),
            }]
        );

        v.add_virtual_supply(0, 0.into());

        assert_eq!(
            v.entries,
            vec![Entry {
                snapshot_index: 0,
                add_virtual_credit: 0.into(),
                add_market_virtual: 0.into(),
            }],
            "merge identical zero entries"
        );

        v.add_virtual_supply(3, 0.into());
        v.add_virtual_supply(5, 0.into());

        assert_eq!(
            v.entries,
            vec![Entry {
                snapshot_index: 5,
                add_virtual_credit: 0.into(),
                add_market_virtual: 0.into(),
            }],
            "merge zero entries to highest snapshot"
        );

        v.add_virtual_supply(6, 60.into());
        v.add_virtual_supply(7, 70.into());

        assert_eq!(
            v.entries,
            vec![Entry {
                snapshot_index: 7,
                add_virtual_credit: 0.into(),
                add_market_virtual: 130.into(),
            }],
            "merge entries with zero virtual credit to highest snapshot"
        );

        v.add_virtual_credit(8, 80.into());

        assert_eq!(
            v.entries,
            vec![Entry {
                snapshot_index: 8,
                add_virtual_credit: 80.into(),
                add_market_virtual: 130.into(),
            }],
            "add_market_virtual is evaluated before add_virtual_credit"
        );

        v.add_virtual_supply(8, 10.into());

        assert_eq!(
            v.entries,
            vec![Entry {
                snapshot_index: 8,
                add_virtual_credit: 80.into(),
                add_market_virtual: 140.into(),
            }],
            "adding more virtual supply in same snapshot is ok"
        );

        v.add_virtual_supply(9, 10.into());

        assert_eq!(
            v.entries,
            vec![
                Entry {
                    snapshot_index: 8,
                    add_virtual_credit: 80.into(),
                    add_market_virtual: 140.into(),
                },
                Entry {
                    snapshot_index: 9,
                    add_virtual_credit: 0.into(),
                    add_market_virtual: 10.into(),
                },
            ],
            "next snapshot after credit forces new entry"
        );

        v.add_virtual_supply(9, 10.into());

        assert_eq!(
            v.entries,
            vec![
                Entry {
                    snapshot_index: 8,
                    add_virtual_credit: 80.into(),
                    add_market_virtual: 140.into(),
                },
                Entry {
                    snapshot_index: 9,
                    add_virtual_credit: 0.into(),
                    add_market_virtual: 20.into(),
                },
            ],
            "adding virtual supply works as normal"
        );

        v.add_virtual_supply(9, 10.into());

        assert_eq!(
            v.entries,
            vec![
                Entry {
                    snapshot_index: 8,
                    add_virtual_credit: 80.into(),
                    add_market_virtual: 140.into(),
                },
                Entry {
                    snapshot_index: 9,
                    add_virtual_credit: 0.into(),
                    add_market_virtual: 20.into(),
                },
            ],
            "adding virtual supply works as normal"
        );
    }

    #[rstest::rstest]
    fn multiple_positions(
        #[values(
            &[Cmd::VirtualSupply(100), Cmd::VirtualSupply(10), Cmd::Credit(50), Cmd::Credit(50), Cmd::VirtualSupply(20)],
            &[Cmd::VirtualSupply(100), Cmd::Credit(10), Cmd::Credit(50), Cmd::VirtualSupply(50), Cmd::VirtualSupply(20), Cmd::Credit(40)],
        )]
        ops: &[Cmd],
        #[values(&[50, 40, 10])] positions: &[u128],
    ) {
        let mut v = VirtualCredit::default();

        for (i, op) in ops.iter().enumerate() {
            match op {
                Cmd::VirtualSupply(supply) => {
                    v.add(i as u32, 0.into(), (*supply).into());
                }
                Cmd::Credit(credit) => {
                    v.add(i as u32, (*credit).into(), 0.into());
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
        v.add(1, 0.into(), 50.into());
        v.add(2, 10.into(), 50.into());
        v.add(3, 100.into(), 0.into());
        v.add(4, 0.into(), 50.into());
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
