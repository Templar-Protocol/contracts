use super::*;

#[test]
fn amortization() {
    let mut a = Accumulator::<crate::data::asset::BorrowAsset>::new(1);

    a.accumulate(AccumulationRecord {
        amount: 100.into(),
        fraction_as_u128_dividend: 0,
        next_snapshot_index: 2,
    });

    assert_eq!(a.get_total(), 100.into());

    a.amortize(25.into());

    assert_eq!(a.get_total(), 125.into());

    a.accumulate(AccumulationRecord {
        amount: 100.into(),
        fraction_as_u128_dividend: 0,
        next_snapshot_index: 3,
    });

    assert_eq!(a.get_total(), 200.into());
}

#[test]
fn fraction() {
    let mut a = Accumulator::<crate::data::asset::BorrowAsset>::new(1);

    a.accumulate(AccumulationRecord {
        amount: 100.into(),
        fraction_as_u128_dividend: 1 << 127,
        next_snapshot_index: 2,
    });

    assert_eq!(a.get_total(), 100.into());

    a.accumulate(AccumulationRecord {
        amount: 100.into(),
        fraction_as_u128_dividend: 1 << 127,
        next_snapshot_index: 3,
    });

    assert_eq!(a.get_total(), 201.into());
}
