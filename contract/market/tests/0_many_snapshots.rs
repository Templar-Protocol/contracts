// This test is particularly long-running. Since tests are run in lexographical
// order, this test is named 0_... to start it running sooner.

use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;
use templar_common::time_chunk::TimeChunkConfiguration;
use test_utils::*;

#[allow(clippy::pedantic)]
fn linear_regression_slope(data: &[(f64, f64)]) -> f64 {
    let n = data.len() as f64;
    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    let mut sum_xy = 0.0;
    let mut sum_xx = 0.0;

    for &(x, y) in data {
        sum_x += x;
        sum_y += y;
        sum_xy += x * y;
        sum_xx += x * x;
    }

    (n * sum_xy - sum_x * sum_y) / (n * sum_xx - sum_x * sum_x)
}

#[rstest]
#[tokio::test]
async fn many_snapshots(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
        extract(c)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.time_chunk_configuration = TimeChunkConfiguration::new(1);
        })
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 100_000),
        c.collateralize(&borrow_user, 200_000),
    );
    let r = c.borrow(&borrow_user, 100).await;
    let base_gas = r.total_gas_burnt.as_gas();
    eprintln!("Base gas: {base_gas}");

    let mut gas_record = vec![];

    // 256 is 2*128 (2 * the chunk size of the snapshots container)
    for i in 0..256 {
        let e = c.borrow(&borrow_user, 100).await;
        let gas = e.total_gas_burnt.as_gas();
        #[allow(clippy::cast_precision_loss)]
        gas_record.push((f64::from(i), gas as f64));
    }

    eprintln!("Base gas:\t{base_gas}");
    for (i, g) in &gas_record {
        eprintln!("Gas {i}:\t{g}");
    }

    let slope = linear_regression_slope(&gas_record);
    eprintln!("Slope: {slope}");

    assert!(slope < 1e+10, "Gas growing with snapshots");
}
