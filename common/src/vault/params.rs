use super::{buffer, near, Gas};

pub const DAY_NS: u64 = 86_400_000_000_000;
pub const YEAR_NS: u64 = 365 * DAY_NS;

pub const MIN_TIMELOCK_NS: u64 = 0;
pub const MAX_TIMELOCK_NS: u64 = 30 * DAY_NS;
pub const MAX_QUEUE_LEN: usize = 64;

#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelockKind {
    Guardian,
    Sentinel,
    Config,
    Cap,
    MarketRemoval,
}

// Fetching a position
const GET_SUPPLY_POSITION: u64 = 4;
pub const GET_SUPPLY_POSITION_GAS: Gas = Gas::from_tgas(GET_SUPPLY_POSITION);

// Create a withdrawal request
pub const CREATE_WITHDRAW_REQ_GAS: Gas = buffer(5);

// Balance reads against the underlying NEP-141
pub const FT_BALANCE_OF_GAS: Gas = Gas::from_tgas(5);

// Idle balance resync (ft_balance_of + callback)
const RESYNC_IDLE_CALLBACK: u64 = 5;
pub const RESYNC_IDLE_CALLBACK_GAS: Gas = buffer(RESYNC_IDLE_CALLBACK);

// 5 TGAS for ft_balance_of + callback buffer
pub const RESYNC_IDLE_GAS: Gas = buffer(5 + RESYNC_IDLE_CALLBACK);

// Execute the next withdrawal request on a market
const EXECUTE_NEXT_SUPPLY_WITHDRAW_REQ: u64 = 20;
pub const EXECUTE_NEXT_SUPPLY_WITHDRAW_REQ_GAS: Gas =
    Gas::from_tgas(EXECUTE_NEXT_SUPPLY_WITHDRAW_REQ);

// Extra gas reserved for post-supply verification callbacks, used in
// paths where we want a conservative safety margin beyond the base
// estimate.
pub const SUPPLY_POST_VERIFY_GAS: Gas = Gas::from_tgas(30);

// Callback gas roots for withdraw/supply orchestration.

// Root budget for callbacks after creating a market-side
// supply-withdrawal request. Encodes: create request, read supply
// position and settle withdraw accounting.
pub const WITHDRAW_CREATE_REQUEST_CALLBACK_GAS: Gas =
    buffer(EXECUTE_NEXT_SUPPLY_WITHDRAW_REQ + AFTER_EXECUTE_NEXT_SUPPLY_WITHDRAW_REQ);

// Budget for the final "settle" phase of a withdraw execution:
// reconcile principal and idle_balance, and potentially transition to
// payout or the next market.
const AFTER_EXECUTE_NEXT_WITHDRAW: u64 = 5 + 5 + AFTER_SEND_TO_USER;
pub const WITHDRAW_SETTLE_CALLBACK_GAS: Gas = buffer(AFTER_EXECUTE_NEXT_WITHDRAW);

// Budget for executing the next supply-withdrawal request on a market
// and fetching the updated supply position before the settle step.
const AFTER_EXECUTE_NEXT_SUPPLY_WITHDRAW_REQ: u64 =
    GET_SUPPLY_POSITION + AFTER_EXECUTE_NEXT_WITHDRAW;
pub const WITHDRAW_EXECUTE_FETCH_POSITION_GAS: Gas = buffer(AFTER_EXECUTE_NEXT_SUPPLY_WITHDRAW_REQ);

const AFTER_SUPPLY_2_READ: u64 = 5;
pub const SUPPLY_POSITION_READ_CALLBACK_GAS: Gas = buffer(AFTER_SUPPLY_2_READ);
pub const SUPPLY_AFTER_TRANSFER_CHECK_GAS: Gas = buffer(GET_SUPPLY_POSITION + AFTER_SUPPLY_2_READ);

// NOTE: these are taken after running the contract with the gas report and cieled to next whole TGAS.
pub const SUPPLY_GAS: Gas = buffer(8);
pub const ALLOCATE_GAS: Gas = buffer(20);
pub const WITHDRAW_GAS: Gas = buffer(4);
pub const EXECUTE_WITHDRAW_GAS: Gas = buffer(9);
pub const SUBMIT_CAP_GAS: Gas = buffer(3);

const AFTER_SEND_TO_USER: u64 = 5;
pub const AFTER_SEND_TO_USER_GAS: Gas = Gas::from_tgas(AFTER_SEND_TO_USER);
