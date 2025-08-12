use std::collections::VecDeque;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};
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
struct CleanupTaskController {
    handle: Option<JoinHandle<()>>,
}

impl CleanupTaskController {
    fn new() -> Self {
        Self { handle: None }
    }

    fn start<T: Send + 'static>(
        &mut self,
        connections: Arc<Mutex<VecDeque<InnerConnection<T>>>>,
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

                log::trace!("Current pool size after cleanup: {}", connections.len());
            }
        });

        self.handle = Some(handle);
    }

    fn stop(&mut self) {
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

/// Manager responsible for creating new [`ConnectionManager::Connection`]s or checking existing ones.
pub trait ConnectionManager: Sync + Send {
    /// Type of [`ConnectionManager::Connection`]s that this [`ConnectionManager`] creates and recycles.
    type Connection: Send;

    /// Error that this [`ConnectionManager`] can return when creating and/or recycling [`ConnectionManager::Connection`]s.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Future that resolves to a new [`ConnectionManager::Connection`] when created.
    type CreateFut: Future<Output = Result<Self::Connection, Self::Error>> + Send;

    /// Future that resolves to true if the connection is valid, false otherwise.
    type ValidFut<'a>: Future<Output = bool> + Send
    where
        Self: 'a;

    /// Create a new connection.
    fn create_connection(&self) -> Self::CreateFut;

    /// Check if a connection is valid.
    fn is_valid<'a>(&'a self, connection: &'a mut Self::Connection) -> Self::ValidFut<'a>;
}

/// Connection pool
pub struct ConnectionPool<M>
where
    M: ConnectionManager + Send + Sync + 'static,
{
    connections: Arc<Mutex<VecDeque<InnerConnection<M::Connection>>>>,
    semaphore: Arc<Semaphore>,
    max_size: usize,
    manager: M,
    max_idle_time: Duration,
    connection_timeout: Duration,
    cleanup_controller: Arc<Mutex<CleanupTaskController>>,
}

/// Pooled inner connection, used within the connection pool
struct InnerConnection<T> {
    pub connection: T,
    pub created_at: Instant,
}

/// Pooled managed stream, provided for the outer world usage
pub struct ManagedConnection<M>
where
    M: ConnectionManager + Send + Sync + 'static,
{
    connection: Option<M::Connection>,
    pool: Arc<ConnectionPool<M>>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl<M> ConnectionPool<M>
where
    M: ConnectionManager + Send + Sync + 'static,
{
    /// Create a new connection pool
    pub fn new(
        max_size: Option<usize>,
        max_idle_time: Option<Duration>,
        connection_timeout: Option<Duration>,
        cleanup_config: Option<CleanupConfig>,
        manager: M,
    ) -> Arc<Self> {
        let max_size = max_size.unwrap_or(DEFAULT_MAX_SIZE);
        let max_idle_time = max_idle_time.unwrap_or(DEFAULT_IDLE_TIMEOUT);
        let connection_timeout = connection_timeout.unwrap_or(DEFAULT_CONNECTION_TIMEOUT);
        let cleanup_config = cleanup_config.unwrap_or_default();

        log::info!(
            "Creating connection pool with max_size: {max_size}, idle_timeout: {max_idle_time:?}, connection_timeout: {connection_timeout:?}, cleanup_enabled: {}",
            cleanup_config.enabled
        );

        let connections = Arc::new(Mutex::new(VecDeque::new()));
        let cleanup_controller = Arc::new(Mutex::new(CleanupTaskController::new()));

        let pool = Arc::new(ConnectionPool {
            connections: connections.clone(),
            semaphore: Arc::new(Semaphore::new(max_size)),
            max_size,
            manager,
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

    /// Get a connection from the pool
    pub async fn get_connection(self: Arc<Self>) -> Result<ManagedConnection<M>, PoolError<M::Error>> {
        log::debug!("Attempting to get connection from pool");

        // Use semaphore to limit concurrent connections
        let permit = self.semaphore.clone().acquire_owned().await.map_err(|_| PoolError::PoolClosed)?;

        // Try to get an existing connection from the pool
        {
            let mut connections = self.connections.lock().await;
            loop {
                let Some(mut pooled_conn) = connections.pop_front() else {
                    // No available connection, break the loop
                    break;
                };
                log::trace!("Found existing connection in pool, validating...");
                let age = Instant::now().duration_since(pooled_conn.created_at);
                let valid = self.manager.is_valid(&mut pooled_conn.connection).await;
                if age >= self.max_idle_time {
                    log::debug!("Connection expired (age: {age:?}), discarding");
                } else if !valid {
                    log::warn!("Connection validation failed, discarding invalid connection");
                } else {
                    log::debug!("Reusing existing connection from pool (remaining: {})", connections.len());
                    return Ok(ManagedConnection {
                        connection: Some(pooled_conn.connection),
                        pool: self.clone(),
                        _permit: permit,
                    });
                }
            }
        }

        log::trace!("No valid connection available, creating new connection...");
        // Create new connection
        match timeout(self.connection_timeout, self.manager.create_connection()).await {
            Ok(Ok(connection)) => {
                log::info!("Successfully created new connection");
                Ok(ManagedConnection {
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

impl<M> ConnectionPool<M>
where
    M: ConnectionManager + Send + Sync + 'static,
{
    async fn recycle(&self, connection: M::Connection) {
        let mut connections = self.connections.lock().await;
        if connections.len() < self.max_size {
            connections.push_back(InnerConnection {
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

impl<M> Drop for ManagedConnection<M>
where
    M: ConnectionManager + Send + Sync + 'static,
{
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            let pool = self.pool.clone();
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                log::trace!("Returning connection to pool on drop");
                tokio::task::block_in_place(|| handle.block_on(pool.recycle(connection)));
            } else {
                log::warn!("No tokio runtime available, connection will be dropped");
            }
        }
    }
}

// Generic implementations for AsRef and AsMut
impl<M> AsRef<M::Connection> for ManagedConnection<M>
where
    M: ConnectionManager + Send + Sync + 'static,
{
    fn as_ref(&self) -> &M::Connection {
        self.connection.as_ref().unwrap()
    }
}

impl<M> AsMut<M::Connection> for ManagedConnection<M>
where
    M: ConnectionManager + Send + Sync + 'static,
{
    fn as_mut(&mut self) -> &mut M::Connection {
        self.connection.as_mut().unwrap()
    }
}

// Implement Deref and DerefMut for PooledStream
impl<M> std::ops::Deref for ManagedConnection<M>
where
    M: ConnectionManager + Send + Sync + 'static,
{
    type Target = M::Connection;

    fn deref(&self) -> &Self::Target {
        self.connection.as_ref().unwrap()
    }
}

impl<M> std::ops::DerefMut for ManagedConnection<M>
where
    M: ConnectionManager + Send + Sync + 'static,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.connection.as_mut().unwrap()
    }
}

/// Connection pool errors
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
