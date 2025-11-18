//! Operation tracking for deposit and withdraw requests
//!
//! Provides in-memory tracking of operation status. In production, this should
//! be replaced with persistent storage (Redis, PostgreSQL, etc.)

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::routes::models::{OperationType, StatusResponse};

/// Information about a tracked operation
#[derive(Debug, Clone)]
pub struct OperationInfo {
    /// Request ID
    pub request_id: String,

    /// Type of operation
    pub operation_type: OperationType,

    /// Current status (free-form text)
    pub status: String,

    /// Additional details
    pub details: HashMap<String, String>,

    /// Timestamp when created
    pub created_at: std::time::SystemTime,

    /// Timestamp when last updated
    pub updated_at: std::time::SystemTime,
}

impl OperationInfo {
    /// Create new operation info
    pub fn new(request_id: String, operation_type: OperationType, status: String) -> Self {
        let now = std::time::SystemTime::now();
        Self {
            request_id,
            operation_type,
            status,
            details: HashMap::new(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Add or update a detail
    pub fn add_detail(&mut self, key: String, value: String) {
        self.details.insert(key, value);
        self.updated_at = std::time::SystemTime::now();
    }

    /// Update status
    pub fn update_status(&mut self, status: String) {
        self.status = status;
        self.updated_at = std::time::SystemTime::now();
    }

    /// Convert to StatusResponse
    pub fn to_response(&self) -> StatusResponse {
        StatusResponse {
            request_id: self.request_id.clone(),
            operation_type: self.operation_type,
            status: self.status.clone(),
            details: self.details.clone(),
        }
    }
}

/// In-memory operation tracker
#[derive(Debug, Clone)]
pub struct OperationTracker {
    operations: Arc<RwLock<HashMap<String, OperationInfo>>>,
}

impl OperationTracker {
    /// Create new tracker
    pub fn new() -> Self {
        Self {
            operations: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Track a new operation
    pub fn track(&self, info: OperationInfo) {
        let mut ops = self.operations.write().unwrap();
        ops.insert(info.request_id.clone(), info);
    }

    /// Get operation by request ID
    pub fn get(&self, request_id: &str) -> Option<OperationInfo> {
        let ops = self.operations.read().unwrap();
        ops.get(request_id).cloned()
    }

    /// Update an existing operation
    pub fn update<F>(&self, request_id: &str, updater: F) -> bool
    where
        F: FnOnce(&mut OperationInfo),
    {
        let mut ops = self.operations.write().unwrap();
        if let Some(info) = ops.get_mut(request_id) {
            updater(info);
            true
        } else {
            false
        }
    }

    /// Get all operations (for debugging/admin)
    #[allow(dead_code)]
    pub fn list_all(&self) -> Vec<OperationInfo> {
        let ops = self.operations.read().unwrap();
        ops.values().cloned().collect()
    }

    /// Clean up old operations (older than duration)
    #[allow(dead_code)]
    pub fn cleanup_old(&self, max_age: std::time::Duration) {
        let mut ops = self.operations.write().unwrap();
        let now = std::time::SystemTime::now();
        ops.retain(|_, info| {
            if let Ok(elapsed) = now.duration_since(info.created_at) {
                elapsed < max_age
            } else {
                true
            }
        });
    }
}

impl Default for OperationTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_operation_info_creation() {
        let info = OperationInfo::new(
            "req-123".to_string(),
            OperationType::Deposit,
            "PENDING".to_string(),
        );

        assert_eq!(info.request_id, "req-123");
        assert_eq!(info.status, "PENDING");
        assert!(info.details.is_empty());
    }

    #[test]
    fn test_operation_info_add_detail() {
        let mut info = OperationInfo::new(
            "req-123".to_string(),
            OperationType::Deposit,
            "PENDING".to_string(),
        );

        info.add_detail("tx_hash".to_string(), "0xabc".to_string());
        assert_eq!(info.details.get("tx_hash"), Some(&"0xabc".to_string()));
    }

    #[test]
    fn test_operation_info_update_status() {
        let mut info = OperationInfo::new(
            "req-123".to_string(),
            OperationType::Deposit,
            "PENDING".to_string(),
        );

        info.update_status("COMPLETED".to_string());
        assert_eq!(info.status, "COMPLETED");
    }

    #[test]
    fn test_tracker_track_and_get() {
        let tracker = OperationTracker::new();
        let info = OperationInfo::new(
            "req-123".to_string(),
            OperationType::Deposit,
            "PENDING".to_string(),
        );

        tracker.track(info);

        let retrieved = tracker.get("req-123");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().request_id, "req-123");
    }

    #[test]
    fn test_tracker_get_nonexistent() {
        let tracker = OperationTracker::new();
        let result = tracker.get("nonexistent");
        assert!(result.is_none());
    }

    #[test]
    fn test_tracker_update() {
        let tracker = OperationTracker::new();
        let info = OperationInfo::new(
            "req-123".to_string(),
            OperationType::Deposit,
            "PENDING".to_string(),
        );

        tracker.track(info);

        let updated = tracker.update("req-123", |info| {
            info.update_status("COMPLETED".to_string());
        });

        assert!(updated);

        let retrieved = tracker.get("req-123");
        assert_eq!(retrieved.unwrap().status, "COMPLETED");
    }

    #[test]
    fn test_tracker_update_nonexistent() {
        let tracker = OperationTracker::new();
        let updated = tracker.update("nonexistent", |info| {
            info.update_status("COMPLETED".to_string());
        });

        assert!(!updated);
    }

    #[test]
    fn test_tracker_list_all() {
        let tracker = OperationTracker::new();

        tracker.track(OperationInfo::new(
            "req-1".to_string(),
            OperationType::Deposit,
            "PENDING".to_string(),
        ));

        tracker.track(OperationInfo::new(
            "req-2".to_string(),
            OperationType::Withdraw,
            "COMPLETED".to_string(),
        ));

        let all = tracker.list_all();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_to_response() {
        let mut info = OperationInfo::new(
            "req-123".to_string(),
            OperationType::Deposit,
            "COMPLETED".to_string(),
        );

        info.add_detail("tx_hash".to_string(), "0xabc".to_string());

        let response = info.to_response();
        assert_eq!(response.request_id, "req-123");
        assert_eq!(response.status, "COMPLETED");
        assert_eq!(response.details.get("tx_hash"), Some(&"0xabc".to_string()));
    }
}
