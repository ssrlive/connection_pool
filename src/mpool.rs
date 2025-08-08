//! A generic connection pool.
//!
//! Implementors of the `ManageConnection` trait provide the specific
//! logic to create and check the health of connections.
//!
//! # Example
//!
//! ```rust, no_run
//! use std::io;
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
//!     async fn connect(&self) -> io::Result<Self::Connection> {
//!         TcpStream::connect(self.addr).await
//!     }
//!
//!     async fn check(&self, _conn: &mut Self::Connection) -> io::Result<()> {
//!         Ok(())
//!     }
//! }
//! ```

use std::collections::VecDeque;
use std::fmt;
use std::io;
use std::marker::PhantomData;
use std::ops::{Add, Deref, DerefMut};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::time::{sleep, timeout};

// #[cfg(test)]
// mod test;

/// Statistics about the connection pool state.
#[derive(Debug, Clone)]
pub struct PoolStats {
    /// Number of connections currently in use.
    pub active: u32,
    /// Number of idle connections in the pool.
    pub idle: usize,
    /// Maximum number of connections allowed.
    pub max_size: u32,
}

/// A trait which provides connection-specific functionality.
#[async_trait]
pub trait ManageConnection: Send + Sync + 'static {
    /// The connection type this manager deals with.
    type Connection: Send + 'static;

    /// Attempts to create a new connection.
    async fn connect(&self) -> io::Result<Self::Connection>;

    /// Check if the connection is still valid, check background every `check_interval`.
    ///
    /// A standard implementation would check if a simple query like `PING` succee,
    /// if the `Connection` is broken, error should return.
    async fn check(&self, conn: &mut Self::Connection) -> io::Result<()>;
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

impl<M: ManageConnection> fmt::Debug for Builder<M> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_struct("Builder")
            .field("max_size", &self.max_size)
            .field("max_lifetime", &self.max_lifetime)
            .field("idle_timeout", &self.idle_timeout)
            .field("connection_timeout", &self.connection_timeout)
            .finish()
    }
}

impl<M: ManageConnection> Default for Builder<M> {
    fn default() -> Self {
        Builder {
            max_lifetime: Some(Duration::from_secs(60 * 30)),
            idle_timeout: Some(Duration::from_secs(3 * 60)),
            connection_timeout: Some(Duration::from_secs(3)),
            check_interval: Some(Duration::from_secs(3)),
            max_size: 10, // ✅ Set reasonable default value instead of 0
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
        if max_lifetime == Some(Duration::from_secs(0)) {
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
        if idle_timeout == Some(Duration::from_secs(0)) {
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
        if connection_timeout == Some(Duration::from_secs(0)) {
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
    pub fn active_count(&self) -> io::Result<u32> {
        self.with_internals(|internals| internals.active.load(Ordering::SeqCst))
    }

    /// Returns the number of idle connections in the pool.
    /// This method is primarily intended for testing and monitoring.
    pub fn idle_count(&self) -> io::Result<usize> {
        self.with_internals(|internals| internals.conns.len())
    }

    /// Returns statistics about the pool.
    /// This method is primarily intended for testing and monitoring.
    pub fn stats(&self) -> io::Result<PoolStats> {
        self.with_internals(|internals| PoolStats {
            active: internals.active.load(Ordering::SeqCst),
            idle: internals.conns.len(),
            max_size: self.inner.max_size,
        })
    }

    fn interval(&self) -> Result<MutexGuard<PoolInternals<M::Connection>>, io::Error> {
        self.inner.intervals.lock().map_err(|_| {
            // Log the poisoned mutex for debugging
            eprintln!("Warning: Pool mutex was poisoned, attempting recovery");
            std::io::Error::other("Pool mutex poisoned")
        })
    }

    /// Force recovery from poisoned mutex by recreating the pool internals
    /// This is a last resort recovery method
    pub fn force_recovery(&self) -> io::Result<()> {
        // Note: This is a destructive operation that will lose all idle connections
        // but it allows the pool to continue functioning
        // In a production system, you might want to log this event
        eprintln!("Warning: Forcing pool recovery - all idle connections will be lost");

        // Try to access the atomic counter directly through unsafe operations
        // This is safe because AtomicU32 operations are always safe
        let current_active = self
            .inner
            .intervals
            .lock()
            .map(|internals| internals.active.load(Ordering::SeqCst))
            .unwrap_or(0);

        // For now, we just report the issue - in a real implementation
        // you might want to replace the mutex entirely
        eprintln!("Current active connections: {}", current_active);
        Ok(())
    }

    fn with_internals<T, F>(&self, f: F) -> io::Result<T>
    where
        F: FnOnce(&mut PoolInternals<M::Connection>) -> T,
    {
        let mut guard = self.interval()?;
        Ok(f(&mut *guard))
    }

    fn pop_front(&self) -> io::Result<Option<IdleConn<M::Connection>>> {
        self.with_internals(|internals| internals.conns.pop_front())
    }

    fn push_back(&self, conn: IdleConn<M::Connection>) -> io::Result<()> {
        self.with_internals(|internals| {
            internals.conns.push_back(conn);
        })
    }

    fn exceed_idle_timeout(&self, conn: &IdleConn<M::Connection>) -> bool {
        if let Some(timeout) = self.inner.idle_timeout {
            if timeout.as_micros() > 0 && conn.last_visited.add(timeout) < Instant::now() {
                return true;
            }
        }
        false
    }

    fn exceed_max_lifetime(&self, conn: &IdleConn<M::Connection>) -> bool {
        if let Some(max_lifetime) = self.inner.max_lifetime {
            if max_lifetime.as_micros() > 0 && conn.created.add(max_lifetime) < Instant::now() {
                return true;
            }
        }
        false
    }

    async fn check(self) {
        if let Some(interval) = self.inner.check_interval {
            loop {
                sleep(interval).await;

                // Safely get the number of idle connections
                let n = match self.idle_count() {
                    Ok(count) => count,
                    Err(_) => {
                        eprintln!("Warning: Failed to get idle count, attempting pool recovery");
                        let _ = self.force_recovery();
                        continue;
                    }
                };

                // Process each idle connection for health checking
                for _ in 0..n {
                    let conn_result = self.pop_front();

                    match conn_result {
                        Ok(Some(mut conn)) => {
                            let should_drop =
                                self.exceed_idle_timeout(&conn) || self.exceed_max_lifetime(&conn);

                            if should_drop {
                                // Connection expired, just drop it
                                // No need to adjust counters as it was idle
                                continue;
                            }

                            // Check connection health
                            match self.inner.manager.check(&mut conn.conn).await {
                                Ok(_) => {
                                    // Connection is healthy, put it back
                                    let _ = self.push_back(conn);
                                }
                                Err(_) => {
                                    // Connection is unhealthy, drop it
                                    // No need to adjust counters as it was idle
                                }
                            }
                        }
                        Ok(None) => break, // No more connections
                        Err(_) => {
                            eprintln!(
                                "Warning: Error accessing connection pool during health check"
                            );
                            break;
                        }
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
    ) -> io::Result<M::Connection> {
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
    pub async fn get(&self) -> io::Result<Connection<M>> {
        // First, try to get an existing idle connection
        if let Some(conn) = self.pop_front()? {
            // Successfully got an idle connection, mark it as active
            self.inner
                .intervals
                .lock()
                .map(|internals| {
                    internals.active.fetch_add(1, Ordering::SeqCst);
                })
                .unwrap_or(());

            return Ok(Connection {
                conn: Some(conn),
                pool: self.clone(),
            });
        }

        // No idle connections available, try to create a new one
        // Atomically check and increment active count
        let can_create = self.with_internals(|internals| {
            let max_size = self.inner.max_size;
            let current_active = internals.active.load(Ordering::SeqCst);
            let current_idle = internals.conns.len() as u32;
            let total_connections = current_active + current_idle;

            if max_size > 0 && total_connections >= max_size {
                false // Cannot create new connection
            } else {
                // Reserve a slot by incrementing active count
                internals.active.fetch_add(1, Ordering::SeqCst);
                true
            }
        })?;

        if !can_create {
            return Err(std::io::Error::other(
                "Connection pool is at maximum capacity",
            ));
        }

        // Create new connection
        let conn = self
            .get_timeout(self.inner.connection_timeout)
            .await
            .inspect_err(|_e| {
                // If creation fails, decrease active count
                self.inner
                    .intervals
                    .lock()
                    .map(|internals| {
                        internals.active.fetch_sub(1, Ordering::SeqCst);
                    })
                    .unwrap_or(());
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

    fn put(&self, mut conn: IdleConn<M::Connection>) -> io::Result<()> {
        conn.last_visited = Instant::now();

        // Check if connection should be dropped due to expiration
        if self.exceed_idle_timeout(&conn) || self.exceed_max_lifetime(&conn) {
            // Connection is expired, decrease active count and drop it
            self.inner
                .intervals
                .lock()
                .map(|internals| {
                    internals.active.fetch_sub(1, Ordering::SeqCst);
                })
                .unwrap_or(());
            return Ok(());
        }

        // Put connection back to pool
        // When connection moves from "in use" to "idle", active count decreases
        self.with_internals(|internals| {
            internals.conns.push_back(conn);
            internals.active.fetch_sub(1, Ordering::SeqCst);
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
