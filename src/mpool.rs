//! A generic connection pool.
//!
//! Implementors of the `ManageConnection` trait provide the specific
//! logic to create and check the health of connections.
//!
//! # Example
//!
//! ```rust, no_run
//! use std::net::SocketAddr;
//!
//! use async_trait::async_trait;
//! use mypool::mpool::{ManageConnection, Pool};
//! use tokio::net::TcpStream;
//!
//! struct MyPool {
//!     addr: SocketAddr,
//! }
//!
//! #[async_trait]
//! impl ManageConnection for MyPool {
//!     type Connection = TcpStream;
//!
//!     async fn connect(&self) -> std::io::Result<Self::Connection> {
//!         TcpStream::connect(self.addr).await
//!     }
//!
//!     async fn check(&self, _conn: &mut Self::Connection) -> std::io::Result<()> {
//!         Ok(())
//!     }
//! }
//! ```

use std::collections::VecDeque;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::time::{sleep, timeout};

/// Statistics about the connection pool state.
#[derive(Debug, Clone)]
pub struct PoolStats {
    /// Number of connections currently in use.
    pub active: u32,
    /// Number of idle connections in the pool.
    pub idle: usize,
    /// Total number of connections (active + idle).
    pub total: u32,
    /// Maximum number of connections allowed.
    pub max_size: u32,
}

/// A trait which provides connection-specific functionality.
#[async_trait::async_trait]
pub trait ManageConnection: Send + Sync + 'static {
    /// The connection type this manager deals with.
    type Connection: Send + 'static;

    /// Attempts to create a new connection.
    async fn connect(&self) -> std::io::Result<Self::Connection>;

    /// Check if the connection is still valid, check background every `check_interval`.
    ///
    /// A standard implementation would check if a simple query like `PING` succee,
    /// if the `Connection` is broken, error should return.
    async fn check(&self, conn: &mut Self::Connection) -> std::io::Result<()>;
}

/// A builder for a connection pool.
pub struct Builder<M: ManageConnection> {
    pub max_lifetime: Option<Duration>,
    pub idle_timeout: Option<Duration>,
    pub connection_timeout: Option<Duration>,
    pub max_size: u32,
    pub check_interval: Option<Duration>,
    _pd: PhantomData<M>,
}

impl<M: ManageConnection> std::fmt::Debug for Builder<M> {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        fmt.debug_struct("Builder")
            .field("max_size", &self.max_size)
            .field("max_lifetime", &self.max_lifetime)
            .field("idle_timeout", &self.idle_timeout)
            .field("connection_timeout", &self.connection_timeout)
            .finish()
    }
}

pub const DEFAULT_MAX_LIFETIME: Duration = Duration::from_secs(30 * 60);
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(3 * 60);
pub const DEFAULT_CONNECTION_TIMEOUT: Duration = Duration::from_secs(3);
pub const DEFAULT_CHECK_INTERVAL: Duration = Duration::from_secs(3);
pub const DEFAULT_MAX_SIZE: u32 = 10;

impl<M: ManageConnection> Default for Builder<M> {
    fn default() -> Self {
        Builder {
            max_lifetime: Some(DEFAULT_MAX_LIFETIME),
            idle_timeout: Some(DEFAULT_IDLE_TIMEOUT),
            connection_timeout: Some(DEFAULT_CONNECTION_TIMEOUT),
            check_interval: Some(DEFAULT_CHECK_INTERVAL),
            max_size: DEFAULT_MAX_SIZE,
            _pd: PhantomData,
        }
    }
}

impl<M: ManageConnection> Builder<M> {
    // Constructs a new `Builder`.
    ///
    /// Parameters are initialized with their default values.
    pub fn new() -> Self {
        Builder::default()
    }

    /// Sets the maximum lifetime of connections in the pool.
    ///
    /// If a connection reaches its maximum lifetime while checked out it will
    /// be closed when it is returned to the pool.
    ///
    /// Defaults to 30 minutes.
    ///
    /// use default if `max_lifetime` is the zero `Duration`.
    pub fn max_lifetime(mut self, max_lifetime: Option<Duration>) -> Self {
        if max_lifetime == Some(Duration::ZERO) {
            self
        } else {
            self.max_lifetime = max_lifetime;
            self
        }
    }

    /// Sets the idle timeout used by the pool.
    ///
    /// If set, connections will be closed after exceed idle time.
    ///
    /// Defaults to 3 minutes.
    ///
    /// use default if `idle_timeout` is the zero `Duration`.
    pub fn idle_timeout(mut self, idle_timeout: Option<Duration>) -> Self {
        if idle_timeout == Some(Duration::ZERO) {
            self
        } else {
            self.idle_timeout = idle_timeout;
            self
        }
    }

    /// Sets the connection timeout used by the pool.
    ///
    /// Calls to `Pool::get` will wait this long for a connection to become
    /// available before returning an error.
    ///
    /// Defaults to 3 seconds.
    /// don't timeout if `connection_timeout` is the zero duration
    pub fn connection_timeout(mut self, connection_timeout: Option<Duration>) -> Self {
        if connection_timeout == Some(Duration::ZERO) {
            self
        } else {
            self.connection_timeout = connection_timeout;
            self
        }
    }

    /// Sets the maximum number of connections managed by the pool.
    ///
    /// Defaults to 10.
    ///
    /// no limited if `max_size` is 0.
    pub fn max_size(mut self, max_size: u32) -> Self {
        self.max_size = max_size;
        self
    }

    /// Sets the check interval of connections managed by the pool use the `ManageConnection::check`.
    ///
    /// Defaults to 3s.
    pub fn check_interval(mut self, interval: Option<Duration>) -> Self {
        self.check_interval = interval;
        self
    }

    /// Consumes the builder, returning a new, initialized pool.
    pub fn build(&self, manager: M) -> Pool<M> {
        let intervals = PoolInternals {
            conns: VecDeque::new(),
            active: AtomicU32::new(0),
        };

        let shared = SharedPool {
            intervals: Mutex::new(intervals),
            max_lifetime: self.max_lifetime,
            idle_timeout: self.idle_timeout,
            connection_timeout: self.connection_timeout,
            max_size: self.max_size,
            check_interval: self.check_interval,
            manager,
        };

        let inner = Arc::new(shared);

        // Create background check task
        let check_pool = Pool {
            inner: inner.clone(),
            _shutdown_handle: None,
        };

        let shutdown_handle = if self.check_interval.is_some() {
            Some(tokio::spawn(check_pool.check()))
        } else {
            None
        };

        Pool {
            inner,
            _shutdown_handle: shutdown_handle,
        }
    }
}

/// A smart pointer wrapping a connection.
pub struct Connection<M: ManageConnection> {
    conn: Option<IdleConn<M::Connection>>,
    pool: Pool<M>,
}

impl<M: ManageConnection> Drop for Connection<M> {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            // Ignore errors when returning connection, as they cannot be handled in Drop
            let _ = self.pool.put(conn);
        }
    }
}

impl<M: ManageConnection> Deref for Connection<M> {
    type Target = M::Connection;

    fn deref(&self) -> &M::Connection {
        &self.conn.as_ref().unwrap().conn
    }
}

impl<M: ManageConnection> DerefMut for Connection<M> {
    fn deref_mut(&mut self) -> &mut M::Connection {
        &mut self.conn.as_mut().unwrap().conn
    }
}

/// A generic connection pool.
pub struct Pool<M: ManageConnection> {
    inner: Arc<SharedPool<M>>,
    _shutdown_handle: Option<tokio::task::JoinHandle<()>>,
}

impl<M: ManageConnection> Clone for Pool<M> {
    fn clone(&self) -> Pool<M> {
        Pool {
            inner: self.inner.clone(),
            _shutdown_handle: None, // Don't copy task handle when cloning
        }
    }
}

impl<M: ManageConnection> Pool<M> {
    /// Creates a new connection pool with a default configuration.
    pub fn new(manager: M) -> Pool<M> {
        Pool::builder().build(manager)
    }

    /// Returns a builder type to configure a new pool.
    pub fn builder() -> Builder<M> {
        Builder::new()
    }

    /// Returns the number of active connections.
    /// This method is primarily intended for testing and monitoring.
    pub fn active_count(&self) -> std::io::Result<u32> {
        self.with_internals(|internals| internals.active.load(Ordering::SeqCst))
    }

    /// Returns the number of idle connections in the pool.
    /// This method is primarily intended for testing and monitoring.
    pub fn idle_count(&self) -> std::io::Result<usize> {
        self.with_internals(|internals| internals.conns.len())
    }

    /// Returns the total number of connections (active + idle).
    /// This method is primarily intended for testing and monitoring.
    pub fn total_count(&self) -> std::io::Result<u32> {
        self.with_internals(|internals| {
            internals.active.load(Ordering::SeqCst) + internals.conns.len() as u32
        })
    }

    /// Check if the pool is at maximum capacity.
    pub fn is_at_capacity(&self) -> std::io::Result<bool> {
        let max_size = self.inner.max_size;
        if max_size == 0 {
            return Ok(false); // No limit
        }

        let total = self.total_count()?;
        Ok(total >= max_size)
    }

    /// Returns statistics about the pool.
    /// This method is primarily intended for testing and monitoring.
    pub fn stats(&self) -> std::io::Result<PoolStats> {
        self.with_internals(|internals| {
            let active = internals.active.load(Ordering::SeqCst);
            let idle = internals.conns.len();
            PoolStats {
                active,
                idle,
                total: active + idle as u32,
                max_size: self.inner.max_size,
            }
        })
    }

    fn interval(&self) -> Result<MutexGuard<PoolInternals<M::Connection>>, std::io::Error> {
        self.inner.intervals.lock().map_err(|e| {
            // Log the poisoned mutex for debugging
            log::warn!("Pool mutex was poisoned: {e}. This indicates a panic occurred while holding the lock.");
            std::io::Error::other("Pool mutex poisoned - pool may be in inconsistent state")
        })
    }

    /// Attempt to recover from a poisoned mutex
    /// This is a last resort recovery method that tries to extract data from poisoned mutex
    pub fn force_recovery(&self) -> std::io::Result<PoolStats> {
        match self.inner.intervals.lock() {
            Ok(internals) => {
                // Mutex is not poisoned, return current stats
                Ok(PoolStats {
                    active: internals.active.load(Ordering::SeqCst),
                    idle: internals.conns.len(),
                    total: internals.active.load(Ordering::SeqCst) + internals.conns.len() as u32,
                    max_size: self.inner.max_size,
                })
            }
            Err(poisoned) => {
                log::warn!("Attempting to recover from poisoned mutex");

                // Access the data inside the poisoned mutex
                let internals = poisoned.into_inner();
                let stats = PoolStats {
                    active: internals.active.load(Ordering::SeqCst),
                    idle: internals.conns.len(),
                    total: internals.active.load(Ordering::SeqCst) + internals.conns.len() as u32,
                    max_size: self.inner.max_size,
                };

                log::debug!("Recovery stats: {stats:?}");
                Ok(stats)
            }
        }
    }

    fn with_internals<T, F>(&self, f: F) -> std::io::Result<T>
    where
        F: FnOnce(&mut PoolInternals<M::Connection>) -> T,
    {
        let mut guard = self.interval()?;
        Ok(f(&mut *guard))
    }

    fn pop_front(&self) -> std::io::Result<Option<IdleConn<M::Connection>>> {
        self.with_internals(|internals| internals.conns.pop_front())
    }

    fn push_back(&self, conn: IdleConn<M::Connection>) -> std::io::Result<()> {
        self.with_internals(|internals| {
            internals.conns.push_back(conn);
        })
    }

    fn exceed_idle_timeout(&self, conn: &IdleConn<M::Connection>) -> bool {
        if let Some(timeout) = self.inner.idle_timeout {
            if timeout.as_micros() > 0 {
                return conn
                    .last_visited
                    .checked_add(timeout)
                    .map(|deadline| deadline < Instant::now())
                    .unwrap_or(true); // If overflow, consider it expired
            }
        }
        false
    }

    fn exceed_max_lifetime(&self, conn: &IdleConn<M::Connection>) -> bool {
        if let Some(max_lifetime) = self.inner.max_lifetime {
            if max_lifetime.as_micros() > 0 {
                return conn
                    .created
                    .checked_add(max_lifetime)
                    .map(|deadline| deadline < Instant::now())
                    .unwrap_or(true); // If overflow, consider it expired
            }
        }
        false
    }

    async fn check(self) {
        if let Some(interval) = self.inner.check_interval {
            let mut consecutive_errors = 0;
            const MAX_CONSECUTIVE_ERRORS: u32 = 5;

            loop {
                sleep(interval).await;

                // Safely get the number of idle connections
                let n = match self.idle_count() {
                    Ok(count) => {
                        consecutive_errors = 0; // Reset error counter on success
                        count
                    }
                    Err(e) => {
                        consecutive_errors += 1;
                        log::warn!(
                            "Failed to get idle count (attempt {consecutive_errors}/{MAX_CONSECUTIVE_ERRORS}): {e}",
                        );

                        if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                            log::error!("Too many consecutive failures, attempting pool recovery");
                            match self.force_recovery() {
                                Ok(stats) => {
                                    log::debug!("Pool recovery successful: {stats:?}");
                                    consecutive_errors = 0;
                                }
                                Err(recovery_err) => {
                                    log::error!("Fatal: Pool recovery failed: {recovery_err}");
                                    break; // Exit check loop if recovery fails
                                }
                            }
                        }
                        continue;
                    }
                };

                // Process connections in smaller batches to reduce lock contention
                const BATCH_SIZE: usize = 5;
                let batches = n.div_ceil(BATCH_SIZE); // Ceiling division

                for batch in 0..batches {
                    let batch_size = std::cmp::min(BATCH_SIZE, n - batch * BATCH_SIZE);

                    for _ in 0..batch_size {
                        let conn_result = self.pop_front();

                        match conn_result {
                            Ok(Some(mut conn)) => {
                                let should_drop = self.exceed_idle_timeout(&conn)
                                    || self.exceed_max_lifetime(&conn);

                                if should_drop {
                                    // Connection expired, just drop it
                                    // Note: This was an idle connection, so no active count change needed
                                    continue;
                                }

                                // Check connection health
                                match self.inner.manager.check(&mut conn.conn).await {
                                    Ok(_) => {
                                        // Connection is healthy, put it back
                                        if let Err(e) = self.push_back(conn) {
                                            log::warn!(
                                                "Failed to return healthy connection to pool: {e}"
                                            );
                                        }
                                    }
                                    Err(check_err) => {
                                        // Connection is unhealthy, drop it
                                        log::debug!("Dropped unhealthy connection: {check_err}");
                                        // Note: This was an idle connection, so no active count change needed
                                    }
                                }
                            }
                            Ok(None) => break, // No more connections in this batch
                            Err(e) => {
                                log::warn!(
                                    "Error accessing connection pool during health check: {e}",
                                );
                                break;
                            }
                        }
                    }

                    // Small delay between batches to allow other operations
                    if batch < batches - 1 {
                        tokio::task::yield_now().await;
                    }
                }
            }
        }
    }

    /// Retrieves a connection from the pool.
    ///
    /// Waits for at most the connection timeout before returning an error.
    pub async fn get_timeout(
        &self,
        connection_timeout: Option<Duration>,
    ) -> std::io::Result<M::Connection> {
        if let Some(connection_timeout) = connection_timeout {
            let conn = match timeout(connection_timeout, self.inner.manager.connect()).await {
                Ok(s) => match s {
                    Ok(s) => s,
                    Err(e) => {
                        return Err(std::io::Error::other(e.to_string()));
                    }
                },
                Err(e) => {
                    return Err(std::io::Error::other(e.to_string()));
                }
            };
            return Ok(conn);
        }
        self.inner.manager.connect().await
    }

    /// Retrieves a connection from the pool.
    ///
    /// Waits for at most the configured connection timeout before returning an
    /// error.
    pub async fn get(&self) -> std::io::Result<Connection<M>> {
        // Attempt to get an existing connection or reserve a slot for new connection
        let operation_result = self.with_internals(|internals| {
            // First, try to get an existing idle connection
            if let Some(conn) = internals.conns.pop_front() {
                // Successfully got an idle connection, mark it as active
                internals.active.fetch_add(1, Ordering::SeqCst);
                return Ok(Some(conn)); // Return existing connection
            }

            // No idle connections available, check if we can create a new one
            let max_size = self.inner.max_size;
            let current_active = internals.active.load(Ordering::SeqCst);
            let current_idle = internals.conns.len() as u32;
            let total_connections = current_active + current_idle;

            if max_size > 0 && total_connections >= max_size {
                return Err(std::io::Error::other(
                    "Connection pool is at maximum capacity",
                ));
            }

            // Reserve a slot by incrementing active count
            internals.active.fetch_add(1, Ordering::SeqCst);
            Ok(None) // Need to create new connection
        })?;

        match operation_result? {
            Some(conn) => {
                // Return existing connection
                Ok(Connection {
                    conn: Some(conn),
                    pool: self.clone(),
                })
            }
            None => {
                // Create new connection
                let conn = self
                    .get_timeout(self.inner.connection_timeout)
                    .await
                    .inspect_err(|_e| {
                        // If creation fails, decrease active count
                        if let Ok(internals) = self.inner.intervals.lock() {
                            internals.active.fetch_sub(1, Ordering::SeqCst);
                        }
                    })?;

                Ok(Connection {
                    conn: Some(IdleConn {
                        conn,
                        last_visited: Instant::now(),
                        created: Instant::now(),
                    }),
                    pool: self.clone(),
                })
            }
        }
    }

    fn put(&self, mut conn: IdleConn<M::Connection>) -> std::io::Result<()> {
        conn.last_visited = Instant::now();

        // Check if connection should be dropped due to expiration
        if self.exceed_idle_timeout(&conn) || self.exceed_max_lifetime(&conn) {
            // Connection is expired, decrease active count and drop it
            // Note: This was an active connection that's being returned, so we decrease active count
            match self.inner.intervals.lock() {
                Ok(internals) => {
                    internals.active.fetch_sub(1, Ordering::SeqCst);
                    log::debug!("Dropped expired connection, active count decreased");
                }
                Err(e) => {
                    log::error!(
                        "Failed to update active count after dropping expired connection: {e}"
                    );
                    return Err(std::io::Error::other(
                        "Mutex poisoned during connection drop",
                    ));
                }
            }
            return Ok(());
        }

        // Put connection back to pool
        // Connection state transition: Active -> Idle
        self.with_internals(|internals| {
            // Add to idle pool
            internals.conns.push_back(conn);
            // Decrease active count as connection is no longer in use
            internals.active.fetch_sub(1, Ordering::SeqCst);
        })
        .map_err(|e| {
            log::error!("Failed to return connection to pool: {e}");
            e
        })
    }
}

pub(crate) struct SharedPool<M: ManageConnection> {
    pub(crate) intervals: Mutex<PoolInternals<M::Connection>>,
    max_lifetime: Option<Duration>,
    idle_timeout: Option<Duration>,
    connection_timeout: Option<Duration>,
    max_size: u32,
    check_interval: Option<Duration>,
    manager: M,
}

struct IdleConn<C> {
    conn: C,
    last_visited: Instant,
    created: Instant,
}

pub struct PoolInternals<C> {
    conns: VecDeque<IdleConn<C>>,
    pub active: AtomicU32,
}
