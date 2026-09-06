//! Durable [`ExecutionStore`] adapters for the OWS runtime.
//!
//! The runtime's core exposes an [`ExecutionStore`] trait that all execution
//! state mutations flow through. This crate provides concrete durable backends:
//!
//! - [`SqliteExecutionStore`] (feature `sqlite`, default): a real SQLite
//!   database file via `rusqlite` (bundled).
//! - `PostgresExecutionStore` (feature `postgres`): PostgreSQL via
//!   `tokio-postgres`.
//! - `RedisExecutionStore` (feature `redis`): Redis via the `redis` crate.
//!
//! Each store stores execution records and their appended lifecycle events so
//! that an execution's state can be inspected or resumed after a restart.
#![allow(clippy::result_large_err)]

#[cfg(feature = "sqlite")]
pub mod sqlite;
#[cfg(feature = "sqlite")]
pub use sqlite::SqliteExecutionStore;

#[cfg(feature = "postgres")]
pub mod postgres;
#[cfg(feature = "postgres")]
pub use postgres::PostgresExecutionStore;

#[cfg(feature = "redis")]
pub mod redis_store;
#[cfg(feature = "redis")]
pub use redis_store::RedisExecutionStore;

pub use ows_runtime_core::{ExecutionRecord, ExecutionStore, StoredEvent};
