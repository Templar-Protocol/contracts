use near_sdk::near;

use crate::{asset::BorrowAssetAmount, number::Decimal};

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct VirtualCredit {
    pub at_snapshot_index: u32,
    pub amount: BorrowAssetAmount,
    pub market_virtual_redeemed: BorrowAssetAmount,
}

impl VirtualCredit {
    pub fn new(at_snapshot_index: u32) -> Self {
        Self {
            at_snapshot_index,
            amount: 0.into(),
            market_virtual_redeemed: 0.into(),
        }
    }

    pub fn take(
        &mut self,
        // diff: &mut VirtualCreditDiff,
        position_virtual: BorrowAssetAmount,
        market_virtual: BorrowAssetAmount,
    ) -> BorrowAssetAmount {
        if market_virtual <= self.market_virtual_redeemed
            || position_virtual.is_zero()
            || self.amount.is_zero()
        {
            return BorrowAssetAmount::zero();
        }

        let entitlement = self.amount.min(position_virtual).min(
            (Decimal::from(self.amount) * u128::from(position_virtual)
                / u128::from(market_virtual - self.market_virtual_redeemed))
            .to_u128_floor()
            .unwrap_or_else(|| {
                crate::panic_with_message(&format!("Invariant violation: position_virtual > market_virtual ({position_virtual} > {market_virtual})"));
            })
            .into(),
        );

        self.market_virtual_redeemed += position_virtual;
        self.amount -= entitlement;

        entitlement
    }
}

fn realizable(
    virtual_credit: BorrowAssetAmount,
    market_virtual: BorrowAssetAmount,
    position_virtual: BorrowAssetAmount,
) -> BorrowAssetAmount {
    if market_virtual.is_zero() || position_virtual.is_zero() || virtual_credit.is_zero() {
        return BorrowAssetAmount::zero();
    }

    virtual_credit.min(position_virtual).min(
        (Decimal::from(virtual_credit) * u128::from(position_virtual)
            / u128::from(market_virtual))
        .to_u128_floor()
        .unwrap_or_else(|| {
            crate::panic_with_message(&format!("Invariant violation: position_virtual > market_virtual ({position_virtual} > {market_virtual})"));
        })
        .into(),
    )
}

#[derive(Copy, Clone, Debug)]
enum AmountDiff {
    Add(BorrowAssetAmount),
    Subtract(BorrowAssetAmount),
}

impl AmountDiff {
    pub fn is_zero(&self) -> bool {
        match self {
            Self::Add(amount) | Self::Subtract(amount) => amount.is_zero(),
        }
    }

    pub fn apply(&self, value: BorrowAssetAmount) -> BorrowAssetAmount {
        match self {
            Self::Add(add) => value + *add,
            Self::Subtract(subtract) => value - *subtract,
        }
    }

    pub fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Add(a), Self::Add(b)) => Self::Add(a + b),
            (Self::Subtract(a), Self::Subtract(b)) => Self::Subtract(a + b),
            (Self::Add(add), Self::Subtract(subtract))
            | (Self::Subtract(subtract), Self::Add(add)) => {
                if add >= subtract {
                    Self::Add(add - subtract)
                } else {
                    Self::Subtract(subtract - add)
                }
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct VcList {
    diffs: Vec<VcSnapshot>,
}

impl VcList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(
        &mut self,
        snapshot_index: u32,
        add_virtual_credit: BorrowAssetAmount,
        add_virtual_supply: BorrowAssetAmount,
    ) {
        if let Some(last) = self
            .diffs
            .last_mut()
            .filter(|d| d.snapshot_index == snapshot_index)
        {
            last.market_virtual
                .merge(AmountDiff::Add(add_virtual_supply));
            last.virtual_credit += add_virtual_credit;
        } else {
            self.diffs.push(VcSnapshot {
                snapshot_index,
                virtual_credit: add_virtual_credit,
                market_virtual: AmountDiff::Add(add_virtual_supply),
            });
        }
    }

    pub fn shrink(&mut self) {
        let (mut new_diffs, carryover) = self.diffs.iter().fold(
            (
                Vec::new(),
                VcSnapshot {
                    snapshot_index: 0,
                    virtual_credit: 0.into(),
                    market_virtual: AmountDiff::Add(0.into()),
                },
            ),
            |(mut new_diffs, mut carryover), diff| {
                if (diff.market_virtual.is_zero() && carryover.market_virtual.is_zero())
                    || (carryover.virtual_credit.is_zero())
                {
                    carryover.snapshot_index = diff.snapshot_index;
                    carryover.market_virtual = carryover.market_virtual.merge(diff.market_virtual);
                    carryover.virtual_credit += diff.virtual_credit;
                    (new_diffs, carryover)
                } else {
                    new_diffs.push(carryover);
                    (new_diffs, diff.clone())
                }
            },
        );

        new_diffs.push(carryover);

        self.diffs = new_diffs;
    }

    pub fn realize(
        &mut self,
        from_snapshot_index: u32,
        until_snapshot_index: u32,
        mut position_virtual: BorrowAssetAmount,
    ) -> (u32, BorrowAssetAmount) {
        if from_snapshot_index > until_snapshot_index {
            crate::panic_with_message(&format!("Invariant violation: from_snapshot_index > until_snapshot_index ({from_snapshot_index} > {until_snapshot_index})"));
        }

        let mut market_virtual = BorrowAssetAmount::zero();
        let mut realized = BorrowAssetAmount::zero();
        let mut last_snapshot_index = from_snapshot_index;

        for diff in &mut self.diffs {
            if diff.snapshot_index >= until_snapshot_index {
                break;
            }

            market_virtual = diff.market_virtual.apply(market_virtual);

            if diff.snapshot_index >= from_snapshot_index {
                let r = diff.virtual_credit.min(position_virtual);

                // dbg!((diff.virtual_credit, market_virtual, position_virtual));

                position_virtual -= r;
                diff.virtual_credit -= r;
                market_virtual -= r;
                diff.market_virtual = diff.market_virtual.merge(AmountDiff::Subtract(r));
                realized += r;

                last_snapshot_index = diff.snapshot_index;
            }
        }

        self.shrink();

        (last_snapshot_index, realized)
    }
}

#[derive(Debug, Clone)]
struct VcSnapshot {
    snapshot_index: u32,
    virtual_credit: BorrowAssetAmount,
    market_virtual: AmountDiff,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[rstest::rstest]
    fn virtual_credit_list_multiple_positions(
        #[values(
            &[
                (0, 100),
                (0, 10),
                (50, 0),
                (50, 0),
                (0, 20),
            ],
            &[
                (0, 10),
                (50, 0),
                (0, 100),
                (50, 0),
                (0, 20),
            ],
        )]
        ops: &[(u128, u128)],
        #[values(&[50, 40, 10])] positions: &[u128],
    ) {
        let mut v = VcList::new();

        for (i, (credit, supply)) in ops.iter().enumerate() {
            v.add(i as u32, (*credit).into(), (*supply).into());
        }

        for position in positions {
            eprintln!("position: {position}");
            eprintln!("{v:#?}");
            let position = BorrowAssetAmount::new(*position);

            let (_s, r) = v.realize(0, ops.len() as u32, position);
            assert_eq!(r, position);
        }
    }

    #[rstest::rstest]
    fn virtual_credit_list_single_position(
        #[values(&[0, 1, 2, 3, 4, 5], &[0, 2, 5], &[0, 5], &[1, 5])] splits: &[u32],
        #[values(1, 25, 50, 100)] position_virtual: u128,
    ) {
        let position_virtual: BorrowAssetAmount = position_virtual.into();

        let mut v = VcList::new();
        v.add(1, 0.into(), 50.into());
        v.add(2, 10.into(), 50.into());
        v.add(3, 100.into(), 0.into());
        v.add(4, 0.into(), 50.into());
        let mut total = BorrowAssetAmount::zero();
        let mut remaining = position_virtual;
        for i in 1..splits.len() {
            let a = splits[i - 1];
            let b = splits[i];
            let (_s, r) = v.realize(a, b, remaining);
            remaining -= r;
            total += r;
        }
        assert_eq!(total, position_virtual);
    }
}
