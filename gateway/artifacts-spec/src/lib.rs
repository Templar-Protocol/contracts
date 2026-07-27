pub mod artifact;

/// Invoke `$callback!($spec)` once for every **read** method served by
/// `templar_gateway_artifacts_dispatch::Dispatch`.
///
/// Add or remove a line here whenever you add or remove a read method.
#[macro_export]
macro_rules! for_each_artifact_read_method {
    ($callback:ident) => {
        $callback!($crate::artifact::GetArtifact);
        $callback!($crate::artifact::ListArtifacts);
    };
}

/// Invoke `$callback!($spec)` once for every **write** method served by
/// `templar_gateway_artifacts_dispatch::Dispatch`.
///
/// Add or remove a line here whenever you add or remove a write method.
#[macro_export]
macro_rules! for_each_artifact_write_method {
    ($callback:ident) => {
        $callback!($crate::artifact::AddArtifactVersion);
    };
}
