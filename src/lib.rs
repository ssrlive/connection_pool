#![doc = include_str!("../README.md")]

mod connection_pool;
pub use connection_pool::{CleanupConfig, ConnectionManager, ConnectionPool, ManagedConnection, PoolError};

#[cfg(feature = "tcp")]
mod tcp;
#[cfg(feature = "tcp")]
pub use tcp::{TcpConnectionManager, TcpConnectionPool, TcpManagedConnection};
