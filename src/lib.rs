#![doc = include_str!("../README.md")]

mod connection_pool;

pub use connection_pool::{
    CleanupConfig, ConnectionCreator, ConnectionPool, ConnectionValidator, PoolError, TcpConnectionPool, TcpPooledStream,
};
