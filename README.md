# Connection Pool

A high-performance, generic async connection pool for Rust with background cleanup and comprehensive logging.

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

## 🚀 Quick Start

Add this to your `Cargo.toml`:

```toml
[dependencies]
connection-pool = "0.1"
tokio = { version = "1.0", features = ["full"] }
```

### Simple TCP Connection Pool

```rust, no_run
use connection_pool::TcpConnectionPool;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a TCP connection pool
    let pool = TcpConnectionPool::new_tcp(
        Some(10),                                    // max connections
        Some(Duration::from_secs(300)),             // idle timeout
        Some(Duration::from_secs(10)),              // connection timeout
        None,                                       // default cleanup config
        "127.0.0.1:8080".to_string(),             // target address
    );

    // Get a connection from the pool
    let mut conn = pool.get_connection().await?;
    
    // Use the connection (auto-derefs to TcpStream)
    conn.write_all(b"Hello, World!").await?;
    
    // Connection automatically returns to pool when dropped
    Ok(())
}
```

## 🏗️ Custom Connection Types

### Database Connection Example

```rust,no_run
use connection_pool::{ConnectionCreator, ConnectionPool, ConnectionValidator};
use std::future::Future;

// Define your connection type
#[derive(Debug)]
pub struct DatabaseConnection {
    pub id: u32,
    pub is_connected: bool,
}

// Define connection parameters
#[derive(Clone)]
pub struct DbParams {
    pub host: String,
    pub port: u16,
    pub database: String,
}

// Implement connection creator
pub struct DbCreator;

impl ConnectionCreator<DatabaseConnection, DbParams> for DbCreator {
    type Error = std::io::Error;
    type Future = std::pin::Pin<Box<dyn Future<Output = Result<DatabaseConnection, Self::Error>> + Send>>;

    fn create_connection(&self, params: &DbParams) -> Self::Future {
        let _params = params.clone();
        Box::pin(async move {
            // Your connection logic here
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            Ok(DatabaseConnection { id: 1, is_connected: true })
        })
    }
}

// Implement connection validator
pub struct DbValidator;

impl ConnectionValidator<DatabaseConnection> for DbValidator {
    async fn is_valid(&self, connection: &DatabaseConnection) -> bool {
        connection.is_connected
    }
}

// Create your custom pool type
type DbPool = ConnectionPool<DatabaseConnection, DbParams, DbCreator, DbValidator>;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let params = DbParams {
        host: "localhost".to_string(),
        port: 5432,
        database: "myapp".to_string(),
    };

    let pool = DbPool::new(
        Some(5),     // max connections
        None,        // default idle timeout
        None,        // default connection timeout
        None,        // default cleanup config
        params,      // connection parameters
        DbCreator,   // connection creator
        DbValidator, // connection validator
    );

    // Use the pool
    let conn = pool.get_connection().await?;
    println!("Connected to database with ID: {}", conn.id);

    Ok(())
}
```

## 🧹 Background Cleanup Configuration

Control the background cleanup behavior for optimal performance:

```rust,ignore
use connection_pool::{TcpConnectionPool, CleanupConfig};
use std::time::Duration;

let cleanup_config = CleanupConfig {
    interval: Duration::from_secs(30),  // cleanup every 30 seconds
    enabled: true,                      // enable background cleanup
};

let pool = TcpConnectionPool::new_tcp(
    Some(10),
    Some(Duration::from_secs(300)),
    Some(Duration::from_secs(10)),
    Some(cleanup_config),
    "127.0.0.1:8080".to_string(),
);

// Runtime control
pool.stop_cleanup_task().await;                    // stop cleanup
pool.restart_cleanup_task(new_config).await;       // restart with new config
```

## 📊 Logging and Observability

Enable comprehensive logging to monitor pool behavior:

```rust,no_run
// Initialize logger (using env_logger)
env_logger::Builder::from_default_env()
    .filter_level(log::LevelFilter::Info)  // or Debug for detailed info
    .init();

// The pool will log:
// - Connection creation and reuse
// - Background cleanup operations  
// - Error conditions and warnings
// - Performance metrics
```

Log levels:
- `ERROR`: Connection creation failures
- `WARN`: Validation failures, runtime issues
- `INFO`: Pool creation, connection lifecycle
- `DEBUG`: Detailed operation info, cleanup results
- `TRACE`: Fine-grained debugging information

## 🎛️ Configuration Options

| Parameter | Description | Default |
|-----------|-------------|---------|
| `max_size` | Maximum number of connections | 10 |
| `max_idle_time` | Connection idle timeout | 5 minutes |
| `connection_timeout` | Connection creation timeout | 10 seconds |
| `cleanup_interval` | Background cleanup interval | 30 seconds |

## 🏗️ Architecture

The connection pool uses a sophisticated multi-layered architecture:

```text
┌─────────────────────────────────────────────────────────┐
│                 ConnectionPool<T,P,C,V>                 │
├─────────────────────────────────────────────────────────┤
│  • Generic over connection type T                       │
│  • Parameterized by P (connection params)               │  
│  • Uses C: ConnectionCreator for connection creation    │
│  • Uses V: ConnectionValidator for health checks        │
└─────────────────────────────────────────────────────────┘
                             │
        ┌────────────────────┼─────────────────────┐
        │                    │                     │
┌───────▼──────┐    ┌────────▼─────────┐    ┌──────▼──────┐
│   Semaphore  │    │ Background       │    │  Connection │
│   (Capacity  │    │ Cleanup Task     │    │    Queue    │
│   Control)   │    │ (Async Worker)   │    │ (VecDeque)  │
└──────────────┘    └──────────────────┘    └─────────────┘
```

### Key Components

- **Semaphore**: Controls maximum concurrent connections
- **Background Cleanup**: Async task for removing expired connections  
- **Connection Queue**: FIFO queue of available connections
- **RAII Wrapper**: `PooledStream` for automatic connection return

## 🔧 Advanced Usage

### Multiple Access Patterns

```rust,ignore
let conn = pool.get_connection().await?;

// Method 1: Auto-deref (most common)
conn.write_all(b"data").await?;

// Method 2: Explicit reference
let tcp_stream: &TcpStream = conn.as_ref();
tcp_stream.write_all(b"data").await?;

// Method 3: Mutable reference
let tcp_stream: &mut TcpStream = conn.as_mut();
tcp_stream.write_all(b"data").await?;
```

### Error Handling

```rust,ignore
use connection_pool::PoolError;

match pool.get_connection().await {
    Ok(conn) => {
        // Use connection
    }
    Err(PoolError::Timeout) => {
        println!("Connection creation timed out");
    }
    Err(PoolError::PoolClosed) => {
        println!("Pool has been closed");  
    }
    Err(PoolError::Creation(e)) => {
        println!("Failed to create connection: {}", e);
    }
}
```

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
