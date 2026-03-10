use std::env;
use std::hint::black_box;
use std::time::{Duration, Instant};

use templar_vault_kernel::compute_fee_shares;
use templar_vault_kernel::math::number::Number;
use templar_vault_kernel::math::wad::Wad;
use templar_vault_kernel::state::queue::{
    compute_queue_status, compute_settlement, count_satisfiable, PendingWithdrawal,
};
use templar_vault_kernel::types::Address;

fn build_withdrawals(len: usize) -> Vec<PendingWithdrawal> {
    let mut withdrawals = Vec::with_capacity(len);
    for i in 0..len {
        let owner = addr_with_byte(i as u8);
        let receiver = addr_with_byte((i as u8).wrapping_add(1));
        let escrow_shares = 50_000u128 + i as u128;
        let expected_assets = 75_000u128 + (i as u128 * 7);
        let requested_at_ns = i as u64 * 1_000_000_000u64;
        withdrawals.push(PendingWithdrawal::new(
            owner,
            receiver,
            escrow_shares,
            expected_assets,
            requested_at_ns,
        ));
    }
    withdrawals
}

fn addr_with_byte(byte: u8) -> Address {
    [byte; 32]
}

fn run_round(withdrawals: &[PendingWithdrawal], available_assets: u128) -> u128 {
    let status = compute_queue_status(withdrawals.iter());
    let (count, total_assets) = count_satisfiable(withdrawals.iter(), available_assets);

    if withdrawals.is_empty() {
        return black_box(
            status
                .total_expected_assets
                .wrapping_add(status.total_escrow_shares)
                .wrapping_add(total_assets)
                .wrapping_add(count as u128),
        );
    }

    let head = &withdrawals[0];
    let settlement = compute_settlement(
        head.escrow_shares,
        head.expected_assets,
        available_assets.min(head.expected_assets / 2),
    );

    let cur_total_assets = Number::from(1_000_000_000u128);
    let last_total_assets = Number::from(850_000_000u128);
    let performance_fee = Wad::from(Wad::SCALE / 10); // 10%
    let total_supply = Number::from(500_000_000u128);
    let fee_shares = compute_fee_shares(
        cur_total_assets,
        last_total_assets,
        performance_fee,
        total_supply,
    );

    let mut checksum = status.total_expected_assets ^ status.total_escrow_shares;
    checksum = checksum
        .wrapping_add(total_assets)
        .wrapping_add(count as u128)
        .wrapping_add(settlement.to_burn)
        .wrapping_add(settlement.refund)
        .wrapping_add(fee_shares.as_u128_trunc());

    for i in 0..1_000u128 {
        let a = Number::from(1_000_000u128 + i);
        let b = Number::from(750_000u128 + (i * 3));
        let denom = Number::from(1_000_000u128 + (i * 7));
        let floor = Number::mul_div_floor(a, b, denom).as_u128_trunc();
        let wad = Wad::from(Wad::SCALE / 1_000 * ((i % 1_000) + 1));
        let applied = wad.apply_floored(a).as_u128_trunc();
        checksum ^= floor.wrapping_add(applied);
    }

    black_box(checksum)
}

fn percentile(samples: &[Duration], p: f64) -> Duration {
    if samples.is_empty() {
        return Duration::from_secs(0);
    }
    let clamped = if p < 0.0 {
        0.0
    } else if p > 1.0 {
        1.0
    } else {
        p
    };
    let idx = ((samples.len() - 1) as f64 * clamped).round() as usize;
    samples[idx]
}

fn main() {
    let iterations: usize = env::var("KERNEL_PERF_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let queue_len: usize = env::var("KERNEL_PERF_QUEUE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1_000);

    let withdrawals = build_withdrawals(queue_len);
    let available_assets = withdrawals
        .iter()
        .take(queue_len / 2)
        .fold(0u128, |acc, w| acc.saturating_add(w.expected_assets));

    let _ = run_round(&withdrawals, available_assets);

    let mut samples = Vec::with_capacity(iterations);
    let mut checksum = 0u128;
    for _ in 0..iterations {
        let start = Instant::now();
        checksum ^= run_round(&withdrawals, available_assets);
        samples.push(start.elapsed());
    }

    samples.sort();
    let total: Duration = samples.iter().copied().sum();
    let total_secs = total.as_secs_f64();
    let throughput = if total_secs > 0.0 {
        iterations as f64 / total_secs
    } else {
        0.0
    };

    println!(
        "kernel-perf iterations={} queue_len={}",
        iterations, queue_len
    );
    println!("p50={:?}", percentile(&samples, 0.50));
    println!("p95={:?}", percentile(&samples, 0.95));
    println!("p99={:?}", percentile(&samples, 0.99));
    println!("throughput_rounds_per_sec={:.2}", throughput);
    println!("checksum={}", checksum);
}
