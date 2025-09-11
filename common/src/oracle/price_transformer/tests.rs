use crate::dec;

use super::*;

#[test]
fn price_transformation() {
    let transformation = Action::NormalizeNativeLstPrice { decimals: 24 };
    let price_before = pyth::Price {
        price: 1234.into(),
        conf: 4.into(),
        expo: 5,
        publish_time: 0.into(),
    };

    let price_after = transformation
        .apply(price_before, dec!("1.2").mul_pow10(24).unwrap())
        .unwrap();

    assert_eq!(
        price_after,
        pyth::Price {
            price: 1480.into(),
            conf: 5.into(),
            expo: 5,
            publish_time: 0.into(),
        },
    );
}
