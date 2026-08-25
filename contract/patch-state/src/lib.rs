//! Vendored from `near/core-contracts/state-manipulation` (MIT OR Apache-2.0).

#![cfg_attr(target_arch = "wasm32", no_std)]

extern crate alloc;

use alloc::{vec, vec::Vec};
use templar_patch_state_types::{Op, Patch};

#[cfg(target_arch = "wasm32")]
use core::alloc::{GlobalAlloc, Layout};

const INPUT_REGISTER: u64 = 0;
const STORAGE_VALUE_REGISTER: u64 = 3;
const HASH_REGISTER: u64 = 4;
const EVICTED_REGISTER: u64 = 8;

#[derive(Clone, Copy)]
#[repr(u64)]
enum AccountIdRegister {
    Predecessor = 1,
    Current = 2,
}

#[cfg(target_arch = "wasm32")]
const WASM_PAGE_BYTES: usize = 64 * 1024;

#[cfg(target_arch = "wasm32")]
struct OneShotBumpAllocator;

#[cfg(target_arch = "wasm32")]
extern "C" {
    static __heap_base: u8;
}

#[cfg(target_arch = "wasm32")]
static mut NEXT_ALLOCATION: usize = 0;

#[cfg(target_arch = "wasm32")]
unsafe impl GlobalAlloc for OneShotBumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let heap_base = core::ptr::addr_of!(__heap_base) as usize;
        let next = unsafe { NEXT_ALLOCATION }.max(heap_base);
        let alignment = layout.align();
        let start = match next.checked_add(alignment - 1) {
            Some(address) => address & !(alignment - 1),
            None => return core::ptr::null_mut(),
        };
        let end = match start.checked_add(layout.size()) {
            Some(address) => address,
            None => return core::ptr::null_mut(),
        };
        let required_pages = match end.checked_add(WASM_PAGE_BYTES - 1) {
            Some(bytes) => bytes / WASM_PAGE_BYTES,
            None => return core::ptr::null_mut(),
        };
        let current_pages = core::arch::wasm32::memory_size(0);
        if required_pages > current_pages
            && core::arch::wasm32::memory_grow(0, required_pages - current_pages) == usize::MAX
        {
            return core::ptr::null_mut();
        }

        unsafe { NEXT_ALLOCATION = end };
        start as *mut u8
    }

    unsafe fn dealloc(&self, _: *mut u8, _: Layout) {}
}

#[cfg(target_arch = "wasm32")]
#[global_allocator]
static ALLOC: OneShotBumpAllocator = OneShotBumpAllocator;

#[cfg(target_arch = "wasm32")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe { near_sys::panic() }
}

fn panic_utf8(message: &str) -> ! {
    unsafe { near_sys::panic_utf8(message.len() as u64, message.as_ptr() as u64) }
}

fn read_register(register_id: u64) -> Option<Vec<u8>> {
    let len = unsafe { near_sys::register_len(register_id) };
    if len == u64::MAX {
        return None;
    }

    let len = usize::try_from(len).unwrap_or_else(|_| panic_utf8("register length overflow"));
    let mut value = vec![0; len];
    unsafe { near_sys::read_register(register_id, value.as_mut_ptr() as u64) };
    Some(value)
}

fn input() -> Vec<u8> {
    unsafe { near_sys::input(INPUT_REGISTER) };
    read_register(INPUT_REGISTER).unwrap_or_else(|| panic_utf8("missing input"))
}

fn account_id(account: AccountIdRegister) -> Vec<u8> {
    let register_id = account as u64;
    unsafe {
        match account {
            AccountIdRegister::Predecessor => near_sys::predecessor_account_id(register_id),
            AccountIdRegister::Current => near_sys::current_account_id(register_id),
        }
    }
    read_register(register_id).unwrap_or_else(|| panic_utf8("missing account id"))
}

fn storage_read(key: &[u8]) -> Option<Vec<u8>> {
    let found = unsafe {
        near_sys::storage_read(
            key.len() as u64,
            key.as_ptr() as u64,
            STORAGE_VALUE_REGISTER,
        )
    };
    (found != 0).then(|| {
        read_register(STORAGE_VALUE_REGISTER).unwrap_or_else(|| panic_utf8("missing value"))
    })
}

fn storage_write(key: &[u8], value: &[u8]) {
    unsafe {
        near_sys::storage_write(
            key.len() as u64,
            key.as_ptr() as u64,
            value.len() as u64,
            value.as_ptr() as u64,
            EVICTED_REGISTER,
        )
    };
}

fn storage_remove(key: &[u8]) {
    unsafe { near_sys::storage_remove(key.len() as u64, key.as_ptr() as u64, EVICTED_REGISTER) };
}

fn sha256(value: &[u8]) -> [u8; 32] {
    unsafe { near_sys::sha256(value.len() as u64, value.as_ptr() as u64, HASH_REGISTER) };
    read_register(HASH_REGISTER)
        .unwrap_or_else(|| panic_utf8("missing hash"))
        .try_into()
        .unwrap_or_else(|_| panic_utf8("invalid hash length"))
}

fn require(condition: bool, message: &str) {
    if !condition {
        panic_utf8(message);
    }
}

#[no_mangle]
pub extern "C" fn patch() {
    let current_account_id = account_id(AccountIdRegister::Current);
    require(
        account_id(AccountIdRegister::Predecessor) == current_account_id,
        "patch must be called by the target account",
    );

    let patch = Patch::from_borsh_slice(&input()).unwrap_or_else(|_| panic_utf8("invalid patch"));
    require(
        patch.account_id.as_bytes() == current_account_id,
        "patch target does not match current account",
    );

    for operation in patch.ops {
        match operation {
            Op::Set { key, value } => storage_write(&key, &value),
            Op::Remove { key } => storage_remove(&key),
            Op::Expect { key, value } => {
                require(storage_read(&key) == value, "storage expectation failed");
            }
            Op::ExpectHash {
                key,
                sha256: expected,
            } => {
                let value =
                    storage_read(&key).unwrap_or_else(|| panic_utf8("storage hash key missing"));
                require(sha256(&value) == expected, "storage hash mismatch");
            }
        }
    }
}
