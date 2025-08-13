# Connection Pool

A flexible, high-performance, generic async connection pool for Rust with background cleanup and comprehensive logging.

[![Crates.io](https://img.shields.io/crates/v/connection-pool.svg)](https://crates.io/crates/connection-pool)
[![Documentation](https://docs.rs/connection-pool/badge.svg)](https://docs.rs/connection-pool)
[![License](https://img.shields.io/crates/l/connection-pool.svg)](LICENSE)

## ✨ Features

- 🚀 **High Performance**: Background cleanup eliminates connection acquisition latency
- 🔧 **Fully Generic**: Support for any connection type (TCP, Database, HTTP, etc.)
- ⚡ **Async/Await Native**: Built on tokio with modern async Rust patterns
- 🛡️ **Thread Safe**: Concurrent connection sharing with proper synchronization
- 🧹 **Smart Cleanup**: Configurable background task for expired connection removal
- 📊 **Rich Logging**: Comprehensive observability with configurable log levels
- 🔄 **Auto-Return**: RAII-based automatic connection return to pool
- ⏱️ **Timeout Control**: Configurable timeouts for connection creation
- 🎯 **Type Safe**: Compile-time guarantees with zero-cost abstractions
- 🧩 **Extensible**: Custom connection types and validation logic via the `ConnectionManager` trait
- 🌐 **Versatile**: Suitable for TCP, database, and any custom connection pooling

## Quick Start

Add `connection-pool` to your `Cargo.toml`:

```toml
[dependencies]
connection-pool = "0.2"
tokio = { version = "1.47", features = ["full"] }
```
Then you can use the connection pool in your application:

```rust,no_run
// 1. Define your ConnectionManager
use connection_pool::{ConnectionManager, ConnectionPool};
use std::future::Future;
use std::pin::Pin;
use tokio::net::TcpStream;

#[derive(Clone)]
pub struct TcpConnectionManager {
    pub address: std::net::SocketAddr,
}

impl ConnectionManager for TcpConnectionManager {
    type Connection = TcpStream;
    type Error = std::io::Error;
    type CreateFut = Pin<Box<dyn Future<Output = Result<TcpStream, Self::Error>> + Send>>;
    type ValidFut<'a> = Pin<Box<dyn Future<Output = bool> + Send + 'a>>;

    fn create_connection(&self) -> Self::CreateFut {
        let addr = self.address;
        Box::pin(async move { TcpStream::connect(addr).await })
    }

    fn is_valid<'a>(&'a self, conn: &'a mut Self::Connection) -> Self::ValidFut<'a> {
        Box::pin(async move {
            conn.peer_addr().is_ok()
        })
    }
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    // 2. Create a connection pool
    let manager = TcpConnectionManager { address: "127.0.0.1:8080".parse().unwrap() };
    let pool = ConnectionPool::new(
        Some(10), // max pool size
        None,     // default idle timeout
        None,     // default connection timeout
        None,     // default cleanup config
        manager,
    );

    // 3. Acquire and use a connection
    let mut conn = pool.clone().get_connection().await.unwrap();
    use tokio::io::AsyncWriteExt;
    conn.write_all(b"hello").await.unwrap();
    Ok(())
}
```

## Advanced Usage
- You can pool any connection type (e.g. database, API client) by implementing the `ConnectionManager` trait.
- See `examples/db_example.rs` for a custom type example.

## Testing
```bash
cargo test
```

## 🎛️ Configuration Options

| Parameter | Description | Default |
|-----------|-------------|---------|
| `max_size` | Maximum number of connections | 10 |
| `max_idle_time` | Connection idle timeout | 5 minutes |
| `connection_timeout` | Connection creation timeout | 10 seconds |
| `cleanup_interval` | Background cleanup interval | 30 seconds |

## 🏗️ Architecture


The connection pool is now based on a single `ConnectionManager` abstraction:

```text
┌──────────────────────────────────────────────┐
│           ConnectionPool<M: Manager>         │
├──────────────────────────────────────────────┤
│  • Holds a user-defined ConnectionManager    │
│  • Manages a queue of pooled connections     │
│  • Semaphore for max concurrent connections  │
│  • Background cleanup for idle connections   │
└──────────────────────────────────────────────┘
                │
      ┌─────────┴─────────┐
      │                   │
┌─────▼─────┐   ┌─────────▼────────┐
│ Semaphore │   │ Background Task  │
│ (Limits   │   │ (Cleans up idle  │
│  max conn)│   │  connections)    │
└───────────┘   └──────────────────┘
      │
┌─────▼────────────┐
│ Connection Queue │
│ (VecDeque)       │
└──────────────────┘
      │
┌─────▼────────────┐
│  PooledStream    │
│  (RAII wrapper   │
│   auto-returns)  │
└──────────────────┘
```

### Key Components

- **Semaphore**: Controls maximum concurrent connections
- **Background Cleanup**: Async task for removing expired connections  
- **Connection Queue**: FIFO queue of available connections
- **RAII Wrapper**: `PooledStream` for automatic connection return

## 🧪 Testing

Run the examples to see the pool in action:

```bash
# Basic TCP example
cargo run --example echo_example

# Database connection example  
cargo run --example db_example

# Background cleanup demonstration
cargo run --example background_cleanup_example
```

## 🤝 Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## 🔗 Links

- [Documentation](https://docs.rs/connection-pool)
- [Crates.io](https://crates.io/crates/connection-pool)
- [Repository](https://github.com/ssrlive/connection-pool)
