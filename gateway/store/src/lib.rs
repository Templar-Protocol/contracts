//! Durable operation, idempotency, and persistence adapters for the gateway.

mod memory;
mod postgres;

pub use memory::MemoryStore;
pub use postgres::PostgresStore;
