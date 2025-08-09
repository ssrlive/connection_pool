#![doc = include_str!("../README.md")]

mod generic_pool;

pub use generic_pool::{
    CleanupConfig, ConnectionCreator, ConnectionPool, ConnectionValidator, PoolError, TcpConnectionPool, TcpPooledStream,
};
