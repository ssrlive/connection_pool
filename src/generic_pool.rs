use std::collections::VecDeque;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, Semaphore};
use tokio::time::timeout;

/// Connection creator trait
pub trait ConnectionCreator<T, P> {
    type Error;
    type Future: Future<Output = Result<T, Self::Error>>;

    fn create_connection(&self, params: &P) -> Self::Future;
}

/// Connection validator trait
pub trait ConnectionValidator<T> {
    fn is_valid(&self, connection: &T) -> impl Future<Output = bool> + Send;
}

pub struct ConnectionPool<T, P, C, V>
where
    T: Send + 'static,
    P: Send + Sync + Clone + 'static,
    C: Send + Sync + 'static,
    V: Send + Sync + 'static,
{
    connections: Arc<Mutex<VecDeque<PooledConnection<T>>>>,
    semaphore: Arc<Semaphore>,
    max_size: usize,
    connection_params: P,
    connection_creator: C,
    connection_validator: V,
    max_idle_time: Duration,
}

struct PooledConnection<T> {
    connection: T,
    created_at: Instant,
}

pub struct PooledStream<T, P, C, V>
where
    T: Send + 'static,
    P: Send + Sync + Clone + 'static,
    C: Send + Sync + 'static,
    V: Send + Sync + 'static,
{
    connection: Option<T>,
    pool: Arc<ConnectionPool<T, P, C, V>>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl<T, P, C, V> ConnectionPool<T, P, C, V>
where
    C: ConnectionCreator<T, P> + Send + Sync + 'static,
    V: ConnectionValidator<T> + Send + Sync + 'static,
    T: Send + 'static,
    P: Send + Sync + Clone + 'static,
{
    pub fn new(
        max_size: usize,
        connection_params: P,
        connection_creator: C,
        connection_validator: V,
    ) -> Arc<Self> {
        Arc::new(ConnectionPool {
            connections: Arc::new(Mutex::new(VecDeque::new())),
            semaphore: Arc::new(Semaphore::new(max_size)),
            max_size,
            connection_params,
            connection_creator,
            connection_validator,
            max_idle_time: Duration::from_secs(300), // 5 minutes idle timeout
        })
    }

    pub async fn get_connection(
        self: Arc<Self>,
    ) -> Result<PooledStream<T, P, C, V>, PoolError<C::Error>> {
        // Use semaphore to limit concurrent connections
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| PoolError::PoolClosed)?;

        {
            // Try to get an existing connection from the pool
            let mut connections = self.connections.lock().await;

            self.cleanup_expired_connections(&mut connections).await;

            if let Some(pooled_conn) = connections.pop_front() {
                if self
                    .connection_validator
                    .is_valid(&pooled_conn.connection)
                    .await
                {
                    return Ok(PooledStream {
                        connection: Some(pooled_conn.connection),
                        pool: self.clone(),
                        _permit: permit,
                    });
                }
            }
        }

        // Create new connection
        match timeout(
            Duration::from_secs(10),
            self.connection_creator
                .create_connection(&self.connection_params),
        )
        .await
        {
            Ok(Ok(connection)) => Ok(PooledStream {
                connection: Some(connection),
                pool: self.clone(),
                _permit: permit,
            }),
            Ok(Err(e)) => Err(PoolError::Creation(e)),
            Err(_) => Err(PoolError::Timeout),
        }
    }

    async fn cleanup_expired_connections(&self, connections: &mut VecDeque<PooledConnection<T>>) {
        let now = Instant::now();
        connections.retain(|conn| now.duration_since(conn.created_at) < self.max_idle_time);
    }
}

// Implementation without trait bounds for basic operations
impl<T, P, C, V> ConnectionPool<T, P, C, V>
where
    T: Send + 'static,
    P: Send + Sync + Clone + 'static,
    C: Send + Sync + 'static,
    V: Send + Sync + 'static,
{
    pub async fn return_connection(&self, connection: T) {
        let mut connections = self.connections.lock().await;
        if connections.len() < self.max_size {
            connections.push_back(PooledConnection {
                connection,
                created_at: Instant::now(),
            });
        }
        // If the pool is full, the connection will be dropped (automatically closed)
    }
}

// To maintain compatibility with TcpStream, we provide specific implementations
impl<P, C, V> AsRef<TcpStream> for PooledStream<TcpStream, P, C, V>
where
    P: Send + Sync + Clone + 'static,
    C: Send + Sync + 'static,
    V: Send + Sync + 'static,
{
    fn as_ref(&self) -> &TcpStream {
        self.connection.as_ref().unwrap()
    }
}

impl<P, C, V> AsMut<TcpStream> for PooledStream<TcpStream, P, C, V>
where
    P: Send + Sync + Clone + 'static,
    C: Send + Sync + 'static,
    V: Send + Sync + 'static,
{
    fn as_mut(&mut self) -> &mut TcpStream {
        self.connection.as_mut().unwrap()
    }
}

impl<T, P, C, V> Drop for PooledStream<T, P, C, V>
where
    T: Send + 'static,
    P: Send + Sync + Clone + 'static,
    C: Send + Sync + 'static,
    V: Send + Sync + 'static,
{
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            let pool = self.pool.clone();
            tokio::spawn(async move {
                pool.return_connection(connection).await;
            });
        }
    }
}

// Implement Deref and DerefMut for PooledStream
impl<T, P, C, V> std::ops::Deref for PooledStream<T, P, C, V>
where
    T: Send + 'static,
    P: Send + Sync + Clone + 'static,
    C: Send + Sync + 'static,
    V: Send + Sync + 'static,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.connection.as_ref().unwrap()
    }
}

impl<T, P, C, V> std::ops::DerefMut for PooledStream<T, P, C, V>
where
    T: Send + 'static,
    P: Send + Sync + Clone + 'static,
    C: Send + Sync + 'static,
    V: Send + Sync + 'static,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.connection.as_mut().unwrap()
    }
}

/// Pool errors
#[derive(Debug)]
pub enum PoolError<E> {
    PoolClosed,
    Timeout,
    Creation(E),
}

impl<E: std::fmt::Display> std::fmt::Display for PoolError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PoolError::PoolClosed => write!(f, "Connection pool is closed"),
            PoolError::Timeout => write!(f, "Connection creation timeout"),
            PoolError::Creation(e) => write!(f, "Connection creation failed: {e}"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for PoolError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PoolError::Creation(e) => Some(e),
            _ => None,
        }
    }
}

// Implement for TcpStream
pub struct TcpConnectionCreator;

impl ConnectionCreator<TcpStream, String> for TcpConnectionCreator {
    type Error = std::io::Error;
    type Future = std::pin::Pin<Box<dyn Future<Output = Result<TcpStream, Self::Error>> + Send>>;

    fn create_connection(&self, address: &String) -> Self::Future {
        let addr = address.clone();
        Box::pin(async move { TcpStream::connect(&addr).await })
    }
}

pub struct TcpConnectionValidator;

impl ConnectionValidator<TcpStream> for TcpConnectionValidator {
    async fn is_valid(&self, stream: &TcpStream) -> bool {
        // Simple validation: check if the stream is readable and writable
        stream
            .ready(tokio::io::Interest::READABLE | tokio::io::Interest::WRITABLE)
            .await
            .is_ok()
    }
}

// Convenience type aliases
pub type TcpConnectionPool =
    ConnectionPool<TcpStream, String, TcpConnectionCreator, TcpConnectionValidator>;
pub type TcpPooledStream =
    PooledStream<TcpStream, String, TcpConnectionCreator, TcpConnectionValidator>;

impl TcpConnectionPool {
    pub fn new_tcp(max_size: usize, address: String) -> Arc<Self> {
        Self::new(
            max_size,
            address,
            TcpConnectionCreator,
            TcpConnectionValidator,
        )
    }
}
