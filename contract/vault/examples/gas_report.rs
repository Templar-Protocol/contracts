#![allow(clippy::unwrap_used, clippy::wildcard_imports)]

use near_sdk::Gas;
use near_sdk_contract_tools::ft::nep141::GAS_FOR_FT_TRANSFER_CALL;

const AFTER_SUPPLY_ENSURE_GAS: Gas = Gas::from_tgas(30);
const GET_SUPPLY_POSITION_GAS: Gas = Gas::from_tgas(4);
const AFTER_SUPPLY_POSITION_CHECK_GAS: Gas = Gas::from_tgas(10);

const CREATE_WITHDRAW_REQ_GAS: Gas = Gas::from_tgas(10);
const EXECUTE_WITHDRAW_REQ_GAS: Gas = Gas::from_tgas(10);
const AFTER_CREATE_WITHDRAW_REQ_GAS: Gas = Gas::from_tgas(20);
const AFTER_EXEC_WITHDRAW_READ_GAS: Gas = Gas::from_tgas(10);

const AFTER_SEND_TO_USER_GAS: Gas = Gas::from_tgas(5);

// Conservative per-tx budgets
const TARGET_GAS_CONSERVATIVE: Gas = Gas::from_tgas(285);
const TARGET_GAS_FULL: Gas = Gas::from_tgas(300);

fn sum_gas(vals: &[Gas]) -> Gas {
    Gas::from_gas(
        vals.iter()
            .fold(0u64, |acc, g| acc.saturating_add(g.as_gas())),
    )
}

fn max_iters(budget: Gas, step: Gas) -> u64 {
    if step.as_gas() == 0 {
        return 0;
    }
    budget.as_gas() / step.as_gas()
}

fn allocation_step_gas() -> Gas {
    // 1 allocation step:
    // - ft_transfer_call to market (supply)
    // - callback after_supply_1_check
    // - market.get_supply_position
    // - callback after_supply_2_read
    sum_gas(&[
        GAS_FOR_FT_TRANSFER_CALL,
        AFTER_SUPPLY_ENSURE_GAS,
        GET_SUPPLY_POSITION_GAS,
        AFTER_SUPPLY_POSITION_CHECK_GAS,
    ])
}

fn withdraw_market_step_gas() -> Gas {
    // 1 withdraw "market step" (not including final payout to user):
    // - create_supply_withdrawal_request + callback
    // - execute_next_supply_withdrawal_request + callback
    // - get_supply_position + callback
    sum_gas(&[
        CREATE_WITHDRAW_REQ_GAS,
        AFTER_CREATE_WITHDRAW_REQ_GAS,
        EXECUTE_WITHDRAW_REQ_GAS,
        AFTER_CREATE_WITHDRAW_REQ_GAS, // used for after_exec_withdraw_req
        GET_SUPPLY_POSITION_GAS,
        AFTER_EXEC_WITHDRAW_READ_GAS,
    ])
}

fn payout_gas() -> Gas {
    // ft_transfer to user + callback
    sum_gas(&[GAS_FOR_FT_TRANSFER_CALL, AFTER_SEND_TO_USER_GAS])
}

fn print_header() {
    println!("## Vault Gas Report");
    println!();
    println!("This report is static and based on the unified gas constants in the vault.");
    println!("It estimates:");
    println!("- Per-allocation-step gas and max steps within 285/300 Tgas.");
    println!("- Per-withdraw-market-step gas (excluding final payout) and payout cost.");
    println!();
}

fn print_constants() {
    println!("### Constants");
    println!();
    println!("| Label | Gas |");
    println!("| ----: | --: |");
    println!("| GAS_FOR_FT_TRANSFER_CALL | {GAS_FOR_FT_TRANSFER_CALL} |");
    println!("| AFTER_SUPPLY_ENSURE_GAS | {AFTER_SUPPLY_ENSURE_GAS} |");
    println!("| GET_SUPPLY_POSITION_GAS | {GET_SUPPLY_POSITION_GAS} |");
    println!("| AFTER_SUPPLY_POSITION_CHECK_GAS | {AFTER_SUPPLY_POSITION_CHECK_GAS} |");
    println!("| CREATE_WITHDRAW_REQ_GAS | {CREATE_WITHDRAW_REQ_GAS} |");
    println!("| EXECUTE_WITHDRAW_REQ_GAS | {EXECUTE_WITHDRAW_REQ_GAS} |");
    println!("| AFTER_CREATE_WITHDRAW_REQ_GAS | {AFTER_CREATE_WITHDRAW_REQ_GAS} |");
    println!("| AFTER_EXEC_WITHDRAW_READ_GAS | {AFTER_EXEC_WITHDRAW_READ_GAS} |");
    println!("| AFTER_SEND_TO_USER_GAS | {AFTER_SEND_TO_USER_GAS} |");
    println!();
}

fn print_allocation_report() {
    println!("### Allocation pipeline");
    println!();

    let per_step = allocation_step_gas();
    let steps_285 = max_iters(TARGET_GAS_CONSERVATIVE, per_step);
    let steps_300 = max_iters(TARGET_GAS_FULL, per_step);

    println!("Per allocation step (approx): {per_step}");
    println!("Max steps within 285 Tgas (conservative): {steps_285}");
    println!("Max steps within 300 Tgas (full): {steps_300}");
    println!();
}

fn print_withdraw_report() {
    println!("### Withdraw pipeline");
    println!();

    let per_market_step = withdraw_market_step_gas();
    let payout = payout_gas();

    let steps_285 = max_iters(TARGET_GAS_CONSERVATIVE, per_market_step);
    let steps_300 = max_iters(TARGET_GAS_FULL, per_market_step);

    println!("Per withdraw market-step (without final payout): {per_market_step}");
    println!("Payout (ft_transfer + callback): {}", payout);
    println!("Max market-steps within 285 Tgas (excl. payout): {steps_285}");
    println!("Max market-steps within 300 Tgas (excl. payout): {steps_300}");
    println!();
}

fn print_summary() {
    let alloc_step = allocation_step_gas();
    let w_step = withdraw_market_step_gas();
    let payout = payout_gas();

    println!("### Summary");
    println!();
    println!("| Item | Gas |");
    println!("| ---: | --: |");
    println!("| allocation_step | {alloc_step} |");
    println!("| withdraw_market_step (no payout) | {w_step} |");
    println!("| payout | {payout} |");
    println!("| budget_conservative | {TARGET_GAS_CONSERVATIVE} |");
    println!("| budget_full | {TARGET_GAS_FULL} |");
    println!();
}

fn main() {
    print_header();
    print_constants();
    print_allocation_report();
    print_withdraw_report();
    print_summary();

    println!("Note:");
    println!("- These are static estimates derived from constant budgets.");
    println!("- Actual on-chain gas may differ slightly due to runtime overhead and receipts.");
    println!("- Use this report to choose safe per-tx iteration counts for allocation/withdraw orchestration.");
}
