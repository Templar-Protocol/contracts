use std::borrow::Borrow;

use templar_common::snapshot::Snapshot;

#[derive(Debug, Clone, Default)]
pub struct PartialSnapshot {
    pub active_real: u128,
    pub active_virtual: u128,
    pub collateral: u128,
    pub borrowed: u128,
}

impl PartialSnapshot {
    fn matches(&self, snapshot: &Snapshot) -> Result<(), String> {
        fn test(name: &str, expected: u128, actual: u128) -> Result<(), String> {
            if expected != actual {
                return Err(format!(
                    "Mismatch `{name}`: expected: {expected}, actual: {actual}"
                ));
            }

            Ok(())
        }

        test(
            "borrow_asset_deposited_active_real",
            self.active_real,
            u128::from(snapshot.borrow_asset_deposited_active_real),
        )?;
        test(
            "borrow_asset_deposited_active_virtual",
            self.active_virtual,
            u128::from(snapshot.borrow_asset_deposited_active_virtual),
        )?;
        test(
            "collateral_asset_deposited",
            self.collateral,
            u128::from(snapshot.collateral_asset_deposited),
        )?;
        test(
            "borrow_asset_borrowed",
            self.borrowed,
            u128::from(snapshot.borrow_asset_borrowed),
        )?;
        Ok(())
    }
}

#[macro_export]
macro_rules! states {
    ($({$($body:tt)*}),*$(,)?) => {
        {
            let mut v = vec![$crate::partial::PartialSnapshot::default()];
            $(
                $crate::states!(@next v { $($body)* });
            )*
            v
        }
    };
    (@next $v:ident {
        $($field:ident $op:tt $value:expr),*$(,)?
    }) => {
        let mut last = $v[$v.len() - 1].clone();
        $(
            last.$field $op $value;
        )*
        $v.push(last);
    };
}

pub fn check(
    states: impl IntoIterator<Item = impl Borrow<PartialSnapshot>>,
    snapshots: impl IntoIterator<Item = impl Borrow<Snapshot>>,
) {
    let mut snapshot_iter = snapshots.into_iter().peekable();

    for (i, state) in states.into_iter().enumerate() {
        let state = state.borrow();
        eprintln!("State {i}: {state:#?}");
        eprintln!("Snapshot: {:#?}", snapshot_iter.peek().map(Borrow::borrow));
        state
            .matches(snapshot_iter.next().unwrap().borrow())
            .unwrap_or_else(|e| panic!("State {i} check failed: {e}"));

        // skip duplicates
        while snapshot_iter
            .peek()
            .is_some_and(|s| state.matches(s.borrow()).is_ok())
        {
            snapshot_iter.next();
        }
    }
}
