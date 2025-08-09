use std::collections::VecDeque;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, Semaphore};
use tokio::task::JoinHandle;
use tokio::time::{interval, timeout};

pub const DEFAULT_MAX_SIZE: usize = 10;
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60); // 5 minutes
pub const DEFAULT_CONNECTION_TIMEOUT: Duration = Duration::from_secs(10); // 10 seconds
pub const DEFAULT_CLEANUP_INTERVAL: Duration = Duration::from_secs(30); // 30 seconds

/// Configuration for background cleanup task
#[derive(Clone)]
pub struct CleanupConfig {
    pub interval: Duration,
    pub enabled: bool,
}

impl Default for CleanupConfig {
    fn default() -> Self {
        Self {
            interval: DEFAULT_CLEANUP_INTERVAL,
            enabled: true,
        }
    }
}

/// Background cleanup task controller
pub struct CleanupTaskController {
    handle: Option<JoinHandle<()>>,
}

impl CleanupTaskController {
    pub fn new() -> Self {
        Self { handle: None }
    }

    pub fn start<T: Send + 'static>(
        &mut self,
        connections: Arc<Mutex<VecDeque<PooledConnection<T>>>>,
        max_idle_time: Duration,
        cleanup_interval: Duration,
    ) {
        if self.handle.is_some() {
            log::warn!("Cleanup task is already running");
            return;
        }

        let handle = tokio::spawn(async move {
            let mut interval_timer = interval(cleanup_interval);
            log::info!("Background cleanup task started with interval: {cleanup_interval:?}");

            loop {
                interval_timer.tick().await;

                let mut connections = connections.lock().await;
                let initial_count = connections.len();
                let now = Instant::now();

                connections.retain(|conn| now.duration_since(conn.created_at) < max_idle_time);

                let removed_count = initial_count - connections.len();
                if removed_count > 0 {
                    log::debug!("Background cleanup removed {removed_count} expired connections");
                }

                // Release the lock
                drop(connections);
            }
        });

        self.handle = Some(handle);
    }

    pub fn stop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
            log::info!("Background cleanup task stopped");
        }
    }
}

impl Drop for CleanupTaskController {
    fn drop(&mut self) {
        self.stop();
    }
}

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
    connection_timeout: Duration,
    cleanup_controller: Arc<Mutex<CleanupTaskController>>,
}

pub struct PooledConnection<T> {
    pub connection: T,
    pub created_at: Instant,
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
        max_size: Option<usize>,
        max_idle_time: Option<Duration>,
        connection_timeout: Option<Duration>,
        cleanup_config: Option<CleanupConfig>,
        connection_params: P,
        connection_creator: C,
        connection_validator: V,
    ) -> Arc<Self> {
        let max_size = max_size.unwrap_or(DEFAULT_MAX_SIZE);
        let max_idle_time = max_idle_time.unwrap_or(DEFAULT_IDLE_TIMEOUT);
        let connection_timeout = connection_timeout.unwrap_or(DEFAULT_CONNECTION_TIMEOUT);
        let cleanup_config = cleanup_config.unwrap_or_default();

        log::info!(
            "Creating connection pool with max_size: {}, idle_timeout: {:?}, connection_timeout: {:?}, cleanup_enabled: {}",
            max_size,
            max_idle_time,
            connection_timeout,
            cleanup_config.enabled
        );

        let connections = Arc::new(Mutex::new(VecDeque::new()));
        let cleanup_controller = Arc::new(Mutex::new(CleanupTaskController::new()));

        let pool = Arc::new(ConnectionPool {
            connections: connections.clone(),
            semaphore: Arc::new(Semaphore::new(max_size)),
            max_size,
            connection_params,
            connection_creator,
            connection_validator,
            max_idle_time,
            connection_timeout,
            cleanup_controller: cleanup_controller.clone(),
        });

        // Start background cleanup task if enabled
        if cleanup_config.enabled {
            tokio::spawn(async move {
                let mut controller = cleanup_controller.lock().await;
                controller.start(connections, max_idle_time, cleanup_config.interval);
            });
        }

        pool
    }

    pub async fn get_connection(self: Arc<Self>) -> Result<PooledStream<T, P, C, V>, PoolError<C::Error>> {
        log::debug!("Attempting to get connection from pool");

        // Use semaphore to limit concurrent connections
        let permit = self.semaphore.clone().acquire_owned().await.map_err(|_| PoolError::PoolClosed)?;

        {
            // Try to get an existing connection from the pool
            let mut connections = self.connections.lock().await;

            // With background cleanup enabled, we can skip the inline cleanup
            // for better performance, but still do a quick validation
            if let Some(pooled_conn) = connections.pop_front() {
                log::trace!("Found existing connection in pool, validating...");

                // Quick check if connection is not obviously expired
                let age = Instant::now().duration_since(pooled_conn.created_at);
                if age >= self.max_idle_time {
                    log::debug!("Connection expired (age: {age:?}), discarding");
                } else if self.connection_validator.is_valid(&pooled_conn.connection).await {
                    log::debug!("Reusing existing connection from pool (remaining: {})", connections.len());
                    return Ok(PooledStream {
                        connection: Some(pooled_conn.connection),
                        pool: self.clone(),
                        _permit: permit,
                    });
                } else {
                    log::warn!("Connection validation failed, discarding invalid connection");
                }
            }
        }

        log::trace!("No valid connection available, creating new connection...");
        // Create new connection
        match timeout(
            self.connection_timeout,
            self.connection_creator.create_connection(&self.connection_params),
        )
        .await
        {
            Ok(Ok(connection)) => {
                log::info!("Successfully created new connection");
                Ok(PooledStream {
                    connection: Some(connection),
                    pool: self.clone(),
                    _permit: permit,
                })
            }
            Ok(Err(e)) => {
                log::error!("Failed to create new connection");
                Err(PoolError::Creation(e))
            }
            Err(_) => {
                log::warn!("Connection creation timed out after {:?}", self.connection_timeout);
                Err(PoolError::Timeout)
            }
        }
    }

    /// Stop the background cleanup task
    pub async fn stop_cleanup_task(&self) {
        let mut controller = self.cleanup_controller.lock().await;
        controller.stop();
    }

    /// Restart the background cleanup task with new configuration
    pub async fn restart_cleanup_task(&self, cleanup_config: CleanupConfig) {
        let mut controller = self.cleanup_controller.lock().await;
        controller.stop();

        if cleanup_config.enabled {
            controller.start(self.connections.clone(), self.max_idle_time, cleanup_config.interval);
        }
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
    async fn return_connection(&self, connection: T) {
        let mut connections = self.connections.lock().await;
        if connections.len() < self.max_size {
            connections.push_back(PooledConnection {
                connection,
                created_at: Instant::now(),
            });
            log::trace!("Connection returned to pool (pool size: {})", connections.len());
        } else {
            log::trace!("Pool is full, dropping connection (max_size: {})", self.max_size);
        }
        // If the pool is full, the connection will be dropped (automatically closed)
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
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                log::trace!("Returning connection to pool on drop");
                tokio::task::block_in_place(|| handle.block_on(pool.return_connection(connection)));
            } else {
                log::warn!("No tokio runtime available, connection will be dropped");
            }
        }
    }
}

// Generic implementations for AsRef and AsMut
impl<T, P, C, V> AsRef<T> for PooledStream<T, P, C, V>
where
    T: Send + 'static,
    P: Send + Sync + Clone + 'static,
    C: Send + Sync + 'static,
    V: Send + Sync + 'static,
{
    fn as_ref(&self) -> &T {
        self.connection.as_ref().unwrap()
    }
}

impl<T, P, C, V> AsMut<T> for PooledStream<T, P, C, V>
where
    T: Send + 'static,
    P: Send + Sync + Clone + 'static,
    C: Send + Sync + 'static,
    V: Send + Sync + 'static,
{
    fn as_mut(&mut self) -> &mut T {
        self.connection.as_mut().unwrap()
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
pub type TcpConnectionPool = ConnectionPool<TcpStream, String, TcpConnectionCreator, TcpConnectionValidator>;
pub type TcpPooledStream = PooledStream<TcpStream, String, TcpConnectionCreator, TcpConnectionValidator>;

impl TcpConnectionPool {
    pub fn new_tcp(
        max_size: Option<usize>,
        max_idle_time: Option<Duration>,
        connection_timeout: Option<Duration>,
        cleanup_config: Option<CleanupConfig>,
        address: String,
    ) -> Arc<Self> {
        log::info!("Creating TCP connection pool for address: {address}");
        Self::new(
            max_size,
            max_idle_time,
            connection_timeout,
            cleanup_config,
            address,
            TcpConnectionCreator,
            TcpConnectionValidator,
        )
    }
}
