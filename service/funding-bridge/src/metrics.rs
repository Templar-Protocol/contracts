//! Prometheus metrics for monitoring service health and performance

use lazy_static::lazy_static;
use prometheus::{
    register_counter_vec, register_gauge_vec, register_histogram_vec, CounterVec, Encoder,
    GaugeVec, HistogramVec, TextEncoder,
};

lazy_static! {
    /// HTTP request counter by endpoint and status
    pub static ref HTTP_REQUESTS_TOTAL: CounterVec = register_counter_vec!(
        "funding_bridge_http_requests_total",
        "Total number of HTTP requests",
        &["endpoint", "method", "status"]
    )
    .unwrap();

    /// HTTP request duration histogram
    pub static ref HTTP_REQUEST_DURATION_SECONDS: HistogramVec = register_histogram_vec!(
        "funding_bridge_http_request_duration_seconds",
        "HTTP request duration in seconds",
        &["endpoint", "method"],
        vec![0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0]
    )
    .unwrap();

    /// Deposit operation counter by status
    pub static ref DEPOSIT_OPERATIONS_TOTAL: CounterVec = register_counter_vec!(
        "funding_bridge_deposit_operations_total",
        "Total number of deposit operations",
        &["status", "chain"]
    )
    .unwrap();

    /// Withdraw operation counter by status
    pub static ref WITHDRAW_OPERATIONS_TOTAL: CounterVec = register_counter_vec!(
        "funding_bridge_withdraw_operations_total",
        "Total number of withdraw operations",
        &["status", "chain"]
    )
    .unwrap();

    /// Chain handler availability gauge
    pub static ref CHAIN_HANDLER_AVAILABLE: GaugeVec = register_gauge_vec!(
        "funding_bridge_chain_handler_available",
        "Chain handler availability (1 = available, 0 = unavailable)",
        &["chain"]
    )
    .unwrap();

    /// Active operations gauge by type
    pub static ref ACTIVE_OPERATIONS: GaugeVec = register_gauge_vec!(
        "funding_bridge_active_operations",
        "Number of active operations",
        &["operation_type", "status"]
    )
    .unwrap();

    /// Treasury balance gauge (requires periodic updates)
    pub static ref TREASURY_BALANCE: GaugeVec = register_gauge_vec!(
        "funding_bridge_treasury_balance",
        "Treasury balance by chain and asset",
        &["chain", "asset"]
    )
    .unwrap();
}

/// Record an HTTP request
pub fn record_http_request(endpoint: &str, method: &str, status: u16, duration_secs: f64) {
    HTTP_REQUESTS_TOTAL
        .with_label_values(&[endpoint, method, &status.to_string()])
        .inc();

    HTTP_REQUEST_DURATION_SECONDS
        .with_label_values(&[endpoint, method])
        .observe(duration_secs);
}

/// Record a deposit operation
pub fn record_deposit(status: &str, chain: &str) {
    DEPOSIT_OPERATIONS_TOTAL
        .with_label_values(&[status, chain])
        .inc();
}

/// Record a withdraw operation
pub fn record_withdraw(status: &str, chain: &str) {
    WITHDRAW_OPERATIONS_TOTAL
        .with_label_values(&[status, chain])
        .inc();
}

/// Update chain handler availability
pub fn set_chain_availability(chain: &str, available: bool) {
    CHAIN_HANDLER_AVAILABLE
        .with_label_values(&[chain])
        .set(if available { 1.0 } else { 0.0 });
}

/// Update treasury balance
pub fn set_treasury_balance(chain: &str, asset: &str, balance: f64) {
    TREASURY_BALANCE
        .with_label_values(&[chain, asset])
        .set(balance);
}

/// Get all metrics in Prometheus text format
pub fn get_metrics() -> String {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buffer = Vec::new();
    encoder.encode(&metric_families, &mut buffer).unwrap();
    String::from_utf8(buffer).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_http_request() {
        record_http_request("/health", "GET", 200, 0.01);
        // Metric should be recorded (no panic)
    }

    #[test]
    fn test_record_deposit() {
        record_deposit("COMPLETED", "near");
        // Metric should be recorded (no panic)
    }

    #[test]
    fn test_set_chain_availability() {
        set_chain_availability("near", true);
        set_chain_availability("ethereum", false);
        // Metrics should be set (no panic)
    }

    #[test]
    fn test_get_metrics() {
        // Record a metric first to ensure output contains our metrics
        record_deposit("COMPLETED", "near");

        let metrics = get_metrics();
        assert!(!metrics.is_empty(), "Metrics output should not be empty");
        assert!(
            metrics.contains("funding_bridge_deposit_operations_total"),
            "Metrics should contain deposit counter"
        );
    }

    #[test]
    fn test_record_deposit_multiple_chains() {
        record_deposit("COMPLETED", "ethereum");
        record_deposit("PENDING", "arbitrum");
        record_deposit("FAILED", "solana");

        let metrics = get_metrics();
        assert!(metrics.contains("funding_bridge_deposit_operations_total"));
    }

    #[test]
    fn test_record_withdraw_multiple_statuses() {
        record_withdraw("SUCCESS", "near");
        record_withdraw("FAILED", "ethereum");
        record_withdraw("PENDING", "solana");

        let metrics = get_metrics();
        assert!(metrics.contains("funding_bridge_withdraw_operations_total"));
    }

    #[test]
    fn test_http_request_different_methods() {
        record_http_request("/deposit", "POST", 200, 0.5);
        record_http_request("/withdraw", "POST", 201, 0.3);
        record_http_request("/health", "GET", 200, 0.001);
        record_http_request("/metrics", "GET", 200, 0.002);

        let metrics = get_metrics();
        assert!(metrics.contains("funding_bridge_http_requests_total"));
        assert!(metrics.contains("funding_bridge_http_request_duration_seconds"));
    }

    #[test]
    fn test_http_request_error_statuses() {
        record_http_request("/deposit", "POST", 400, 0.1);
        record_http_request("/deposit", "POST", 500, 0.2);
        record_http_request("/withdraw", "POST", 404, 0.05);

        let metrics = get_metrics();
        // All status codes should be recorded
        assert!(metrics.contains("funding_bridge_http_requests_total"));
    }

    #[test]
    fn test_treasury_balance_updates() {
        set_treasury_balance("near", "NEAR", 1000.0);
        set_treasury_balance("ethereum", "ETH", 5.5);
        set_treasury_balance("ethereum", "USDC", 10000.0);

        let metrics = get_metrics();
        assert!(metrics.contains("funding_bridge_treasury_balance"));
    }

    #[test]
    fn test_chain_availability_toggle() {
        // Set available
        set_chain_availability("ethereum", true);
        let metrics1 = get_metrics();
        assert!(metrics1.contains("funding_bridge_chain_handler_available"));

        // Set unavailable
        set_chain_availability("ethereum", false);
        let metrics2 = get_metrics();
        assert!(metrics2.contains("funding_bridge_chain_handler_available"));
    }

    #[test]
    fn test_active_operations_gauge() {
        ACTIVE_OPERATIONS
            .with_label_values(&["deposit", "running"])
            .set(3.0);

        ACTIVE_OPERATIONS
            .with_label_values(&["withdraw", "running"])
            .set(1.0);

        let metrics = get_metrics();
        assert!(metrics.contains("funding_bridge_active_operations"));
    }

    #[test]
    fn test_metrics_format_is_prometheus() {
        record_deposit("COMPLETED", "test-chain");
        let metrics = get_metrics();

        // Prometheus format has TYPE and HELP comments
        assert!(metrics.contains("# HELP") || metrics.contains("# TYPE"));
    }

    #[test]
    fn test_http_request_duration_buckets() {
        // Test various durations to ensure histogram buckets work
        record_http_request("/test", "GET", 200, 0.001); // 1ms
        record_http_request("/test", "GET", 200, 0.01); // 10ms
        record_http_request("/test", "GET", 200, 0.1); // 100ms
        record_http_request("/test", "GET", 200, 1.0); // 1s
        record_http_request("/test", "GET", 200, 5.0); // 5s

        let metrics = get_metrics();
        assert!(metrics.contains("funding_bridge_http_request_duration_seconds"));
        // Should contain bucket information
        assert!(metrics.contains("bucket") || metrics.contains("le="));
    }

    #[test]
    fn test_deposit_withdraw_counters_independent() {
        record_deposit("COMPLETED", "ethereum");
        record_withdraw("COMPLETED", "ethereum");

        let metrics = get_metrics();

        // Both counters should exist independently
        assert!(metrics.contains("funding_bridge_deposit_operations_total"));
        assert!(metrics.contains("funding_bridge_withdraw_operations_total"));
    }

    #[test]
    fn test_metrics_endpoint_idempotent() {
        // Record a metric to ensure we have data
        record_deposit("COMPLETED", "test-chain");

        // Getting metrics multiple times should work
        let metrics1 = get_metrics();
        let metrics2 = get_metrics();

        assert!(!metrics1.is_empty());
        assert!(!metrics2.is_empty());
    }

    #[test]
    fn test_treasury_balance_zero() {
        set_treasury_balance("test-chain", "TEST", 0.0);

        let metrics = get_metrics();
        assert!(metrics.contains("funding_bridge_treasury_balance"));
    }

    #[test]
    fn test_treasury_balance_large_value() {
        set_treasury_balance("ethereum", "USDC", 1_000_000_000.0);

        let metrics = get_metrics();
        assert!(metrics.contains("funding_bridge_treasury_balance"));
    }

    #[test]
    fn test_all_metric_types_present() {
        // Record at least one of each metric type
        record_http_request("/test", "GET", 200, 0.1);
        record_deposit("COMPLETED", "chain1");
        record_withdraw("SUCCESS", "chain2");
        set_chain_availability("chain3", true);
        set_treasury_balance("chain4", "TOKEN", 100.0);
        ACTIVE_OPERATIONS
            .with_label_values(&["test", "active"])
            .set(1.0);

        let metrics = get_metrics();

        // Verify all our custom metrics are present
        assert!(metrics.contains("funding_bridge_http_requests_total"));
        assert!(metrics.contains("funding_bridge_http_request_duration_seconds"));
        assert!(metrics.contains("funding_bridge_deposit_operations_total"));
        assert!(metrics.contains("funding_bridge_withdraw_operations_total"));
        assert!(metrics.contains("funding_bridge_chain_handler_available"));
        assert!(metrics.contains("funding_bridge_treasury_balance"));
        assert!(metrics.contains("funding_bridge_active_operations"));
    }
}
