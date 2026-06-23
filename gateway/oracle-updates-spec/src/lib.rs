pub mod oracle;

/// Invoke `$callback!($spec)` once for every gateway method served by
/// [`templar_gateway_oracle_updates_dispatch::Dispatch`]. These are all writes.
/// The canonical list of oracle-update methods: add or remove a line here
/// whenever you add or remove one — see
/// [`templar_gateway_methods_spec::for_each_read_method`] for the rationale.
#[macro_export]
macro_rules! for_each_oracle_update_method {
    ($callback:ident) => {
        $callback!($crate::oracle::UpdatePyth);
        $callback!($crate::oracle::UpdateRedStone);
        $callback!($crate::oracle::UpdatePrices);
    };
}
