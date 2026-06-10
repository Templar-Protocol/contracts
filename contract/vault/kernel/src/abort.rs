pub const OVERFLOW: &str = "arithmetic under/overflow";

#[inline]
pub fn abort_unreachable() -> ! {
    #[cfg(target_arch = "wasm32")]
    {
        core::arch::wasm32::unreachable()
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        panic!("abort_unreachable invoked")
    }
}

#[inline]
pub fn unwrap_abort_option<T>(value: Option<T>, _msg: &'static str) -> T {
    match value {
        Some(value) => value,
        None => abort_unreachable(),
    }
}

#[inline]
pub fn unwrap_abort_result<T, E>(value: Result<T, E>, _msg: &'static str) -> T {
    match value {
        Ok(value) => value,
        Err(_) => abort_unreachable(),
    }
}

#[macro_export]
macro_rules! abort {
    ($msg:expr $(,)?) => {{
        #[cfg(not(target_arch = "wasm32"))]
        {
            panic!($msg)
        }

        #[cfg(target_arch = "wasm32")]
        {
            let _ = $msg;
            $crate::abort::abort_unreachable()
        }
    }};
}

#[macro_export]
macro_rules! unwrap_abort {
    ($value:expr, $msg:expr $(,)?) => {{
        $crate::abort::unwrap_abort_option($value, $msg)
    }};
}

#[macro_export]
macro_rules! unwrap_abort_result {
    ($value:expr, $msg:expr $(,)?) => {{
        $crate::abort::unwrap_abort_result($value, $msg)
    }};
}
