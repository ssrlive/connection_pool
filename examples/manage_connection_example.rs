//! Echo Client Example using ManageConnection trait
//! Before running this example, make sure you have a TCP echo server running on 127.0.0.1:8080
//! You can install and run it to create a simple TCP echo server for testing:
//! ```bash
//! cargo install socks5-impl --git https://github.com/tun2proxy/socks5-impl --example echo-server
//! echo-server -l 127.0.0.1:8080
//! ```

use connection_pool::{ConnectionCreator, ConnectionPool, ConnectionValidator};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// A trait which provides connection-specific functionality.
#[allow(clippy::type_complexity)]
pub trait ManageConnection: Send + Sync + 'static {
    /// The connection type this manager deals with.
    type Connection: Send + 'static;

    /// The connection parameters type.
    type Params: Send + Sync + Clone + 'static;

    /// The error type for connection operations.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Attempts to create a new connection with the given parameters.
    fn connect(&self, params: &Self::Params) -> Pin<Box<dyn Future<Output = Result<Self::Connection, Self::Error>> + Send>>;

    /// Check if the connection is still valid.
    /// Returns true if the connection is healthy, false otherwise.
    fn check(&self, conn: &Self::Connection) -> Pin<Box<dyn Future<Output = bool> + Send>>;
}

/// Adapter to use ManageConnection as ConnectionCreator
#[derive(Clone)]
pub struct ConnectionManagerAdapter<M> {
    manager: M,
}

impl<M> ConnectionManagerAdapter<M> {
    pub fn new(manager: M) -> Self {
        Self { manager }
    }
}

impl<T, M> ConnectionCreator<T, M::Params> for ConnectionManagerAdapter<M>
where
    M: ManageConnection<Connection = T>,
{
    type Error = M::Error;
    type Future = Pin<Box<dyn Future<Output = Result<T, Self::Error>> + Send>>;

    fn create_connection(&self, params: &M::Params) -> Self::Future {
        self.manager.connect(params)
    }
}

impl<T, M> ConnectionValidator<T> for ConnectionManagerAdapter<M>
where
    M: ManageConnection<Connection = T>,
{
    #[allow(refining_impl_trait)]
    fn is_valid(&self, connection: &T) -> Pin<Box<dyn Future<Output = bool> + Send>> {
        self.manager.check(connection)
    }
}

/// Example implementation of ManageConnection for TCP connections
#[derive(Clone)]
pub struct TcpConnectionManager;

impl ManageConnection for TcpConnectionManager {
    type Connection = TcpStream;
    type Params = std::net::SocketAddr;
    type Error = std::io::Error;

    fn connect(&self, params: &Self::Params) -> Pin<Box<dyn Future<Output = Result<Self::Connection, Self::Error>> + Send>> {
        let addr = *params;
        Box::pin(async move { TcpStream::connect(addr).await })
    }

    fn check(&self, stream: &Self::Connection) -> Pin<Box<dyn Future<Output = bool> + Send>> {
        // Create a future that will check the connection health
        // We'll use the socket's peer address to verify the connection is still valid
        // If we can get the peer address, the connection is likely still valid
        // This is a simple but effective check for TCP connections
        let peer_addr_result = stream.peer_addr().is_ok();
        Box::pin(async move { peer_addr_result })
    }
}

/// Enhanced Echo Client using ManageConnection trait with connection pooling
pub struct EchoClient {
    pool: std::sync::Arc<
        ConnectionPool<
            TcpStream,
            std::net::SocketAddr,
            ConnectionManagerAdapter<TcpConnectionManager>,
            ConnectionManagerAdapter<TcpConnectionManager>,
        >,
    >,
}

impl EchoClient {
    pub fn new(addr: std::net::SocketAddr, max_connections: Option<usize>) -> Self {
        let manager = TcpConnectionManager;
        let adapter = ConnectionManagerAdapter::new(manager);

        // Create a connection pool using our ManageConnection adapter
        let pool = ConnectionPool::new(
            max_connections,
            Some(Duration::from_secs(300)), // 5 minutes idle timeout
            Some(Duration::from_secs(10)),  // 10 seconds connection timeout
            None,                           // use default cleanup config
            addr,
            adapter.clone(),
            adapter,
        );

        Self { pool }
    }

    /// Send a message to the echo server and receive the response
    pub async fn echo(&self, message: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        log::info!("Sending echo message: {message}");

        // Get a connection from the pool
        let mut conn = self
            .pool
            .clone()
            .get_connection()
            .await
            .map_err(|e| format!("Failed to get connection: {e}"))?;

        // Send the message
        conn.write_all(message.as_bytes()).await?;
        conn.write_all(b"\n").await?;
        conn.flush().await?;

        // Read the response
        let mut buffer = vec![0; 1024];
        let bytes_read = conn.read(&mut buffer).await?;

        let response = String::from_utf8_lossy(&buffer[..bytes_read]).trim().to_string();
        log::info!("Received echo response: {response}");

        Ok(response)
    }

    /// Send multiple messages concurrently to test connection pooling
    pub async fn echo_concurrent(&self, messages: Vec<String>) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
        let mut handles = vec![];

        for (i, message) in messages.into_iter().enumerate() {
            let client_pool = self.pool.clone();
            let handle = tokio::spawn(async move {
                log::info!("Task {i}: Starting echo for message: {message}");

                let mut conn = client_pool
                    .get_connection()
                    .await
                    .map_err(|e| format!("Task {i}: Failed to get connection: {e}"))?;

                // Send message
                conn.write_all(message.as_bytes()).await?;
                conn.write_all(b"\n").await?;
                conn.flush().await?;

                // Read response
                let mut buffer = vec![0; 1024];
                let bytes_read = conn.read(&mut buffer).await?;
                let response = String::from_utf8_lossy(&buffer[..bytes_read]).trim().to_string();

                log::info!("Task {i}: Received response: {response}");
                Ok::<String, Box<dyn std::error::Error + Send + Sync>>(response)
            });
            handles.push(handle);
        }

        let mut results = vec![];
        for handle in handles {
            results.push(handle.await??);
        }

        Ok(results)
    }
}

/// Example of using the ManageConnection trait with Echo Client
pub async fn run_echo_client_example() -> std::io::Result<()> {
    let addr: std::net::SocketAddr = "127.0.0.1:8080".parse().unwrap();

    println!("\nCreating Echo Client with connection pooling...");
    let client = EchoClient::new(addr, Some(5)); // Max 5 connections

    // Test single echo
    println!("\n=== Testing Single Echo ===");
    match client.echo("Hello, Echo Server!").await {
        Ok(response) => println!("Echo response: {response}"),
        Err(e) => println!("Echo failed: {e}"),
    }

    // Test concurrent echoes
    println!("\n=== Testing Concurrent Echoes ===");
    let messages = vec![
        "Message 1".to_string(),
        "Message 2".to_string(),
        "Message 3".to_string(),
        "Message 4".to_string(),
        "Message 5".to_string(),
        "Message 6".to_string(),
        "Message 7".to_string(),
        "Message 8".to_string(),
    ];

    match client.echo_concurrent(messages).await {
        Ok(responses) => {
            println!("Received {} responses:", responses.len());
            for (i, response) in responses.iter().enumerate() {
                println!("  Response {}: {response}", i + 1);
            }
        }
        Err(e) => println!("Concurrent echo failed: {e}"),
    }

    println!("\nEcho client example completed successfully!");
    Ok(())
}

/// Basic example of using ManageConnection trait directly
pub async fn run_basic_manage_connection_example() -> std::io::Result<()> {
    println!("\n=== Basic ManageConnection Example ===");

    // Create a connection manager
    let manager = TcpConnectionManager;
    let adapter = ConnectionManagerAdapter::new(manager);

    let addr: std::net::SocketAddr = "127.0.0.1:8080".parse().unwrap();

    match adapter.create_connection(&addr).await {
        Ok(mut conn) => {
            println!("Successfully created connection to {addr}");

            // Use the adapter as a ConnectionValidator
            let is_valid = adapter.is_valid(&conn).await;
            println!("Connection is valid: {is_valid}");

            conn.write_all(b"<message>").await?;
            conn.flush().await?;

            let mut buffer = vec![0; 1024];
            let bytes_read = conn.read(&mut buffer).await?;
            let response = String::from_utf8_lossy(&buffer[..bytes_read]).trim().to_string();
            println!("Received response: {response}");
        }
        Err(e) => {
            println!("Failed to create connection: {e}");
        }
    }

    println!("Basic ManageConnection example completed");
    Ok(())
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    env_logger::Builder::from_default_env().filter_level(log::LevelFilter::Info).init();

    println!("=== Running Echo Client with ManageConnection Example ===");
    println!("Make sure you have an echo server running on 127.0.0.1:8080");
    println!("You can install and run: cargo install socks5-impl --git https://github.com/tun2proxy/socks5-impl --example echo-server");
    println!("Then run: echo-server -l 127.0.0.1:8080\n");

    run_basic_manage_connection_example().await?;

    // Run the echo client example
    if let Err(e) = run_echo_client_example().await {
        println!("Echo client example failed: {e}");
    }

    Ok(())
}
