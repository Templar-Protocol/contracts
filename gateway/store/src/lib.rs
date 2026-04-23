//! Durable operation, idempotency, and persistence adapters for the gateway.

mod memory;
mod postgres;

pub use memory::MemoryOperationStore;
pub use postgres::PostgresStore;
