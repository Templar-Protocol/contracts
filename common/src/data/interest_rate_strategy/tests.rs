use std::ops::Div;

use crate::dec;

use super::*;

#[test]
fn piecewise() {
    let s = Piecewise::new(Decimal::ZERO, dec!("0.9"), dec!("0.035"), dec!("0.6")).unwrap();

    assert!(s.at(Decimal::ZERO).near_equal(Decimal::ZERO));
    assert!(s.at(dec!("0.1")).near_equal(dec!("0.0035")));
    assert!(s.at(dec!("0.5")).near_equal(dec!("0.0175")));
    assert!(s.at(dec!("0.6")).near_equal(dec!("0.021")));
    assert!(s.at(dec!("0.9")).near_equal(dec!("0.0315")));
    assert!(s.at(dec!("0.95")).near_equal(dec!("0.0615")));
    assert!(s.at(Decimal::ONE).near_equal(dec!("0.0915")));
}

#[test]
fn exponential2() {
    let s = Exponential2::new(dec!("0.005"), dec!("0.08"), dec!("6")).unwrap();
    assert!(s.at(Decimal::ZERO).near_equal(dec!("0.005")));
    assert!(s.at(dec!("0.25")).near_equal(dec!(
        "0.00717669895803117868762306839097547161564207589375463826946828509045412494"
    )));
    assert!(s.at(Decimal::ONE_HALF).near_equal(Decimal::ONE.div(75u32)));
}
