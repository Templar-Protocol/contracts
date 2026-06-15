pub struct ProxyOracle;
pub type ProxyOracleVersion = super::Version<ProxyOracle>;

impl ProxyOracleVersion {
    /// Whether the proxy oracle is the kernelized contract (`>= 0.2.0`), which
    /// returns `Proxy<Source>` from `get_proxy` and delegates governance to a
    /// separate contract. Versions below `0.2.0` are the legacy contract whose
    /// `get_proxy` returns the pre-kernel `v0::Proxy` shape.
    pub fn proxy_is_kernelized(self) -> bool {
        self >= (0, 2, 0)
    }
}
