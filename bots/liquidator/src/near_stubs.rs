// NEAR VM function stubs for native binary builds
// Provides runtime functions normally supplied by the NEAR VM

#![allow(non_snake_case)]

use std::process;

/// NEAR SDK panic handler
#[no_mangle]
pub extern "C" fn panic_utf8(msg_ptr: *const u8, msg_len: u64) {
    let msg = if !msg_ptr.is_null() && msg_len > 0 {
        unsafe {
            #[allow(clippy::cast_possible_truncation)]
            let slice = std::slice::from_raw_parts(msg_ptr, msg_len as usize);
            String::from_utf8_lossy(slice).into_owned()
        }
    } else {
        String::from("(empty panic message)")
    };

    eprintln!("NEAR panic: {msg}");
    process::exit(1);
}
