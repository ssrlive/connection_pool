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
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::time::{sleep, timeout};

// #[cfg(test)]
// mod test;

/// Statistics about the connection pool state.
#[derive(Debug, Clone)]
pub struct PoolStats {
    /// Number of active connections currently in use.
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
            active: 0,
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
        self.with_internals(|internals| internals.active)
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
            active: internals.active,
            idle: internals.conns.len(),
            max_size: self.inner.max_size,
        })
    }

    fn interval(&self) -> Result<MutexGuard<PoolInternals<M::Connection>>, io::Error> {
        self.inner
            .intervals
            .lock()
            .map_err(|_| std::io::Error::other("Pool mutex poisoned"))
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
                    Err(_) => break, // Mutex is poisoned, exit check loop
                };

                for _ in 0..n {
                    let conn_result = self.pop_front();

                    match conn_result {
                        Ok(Some(mut conn)) => {
                            if self.exceed_idle_timeout(&conn) || self.exceed_max_lifetime(&conn) {
                                // Connection expired, decrease active count and drop connection
                                let _ = self.with_internals(|internals| internals.active -= 1);
                                continue;
                            }

                            match self.inner.manager.check(&mut conn.conn).await {
                                Ok(_) => {
                                    // Connection is valid, put it back to pool
                                    let _ = self.push_back(conn);
                                }
                                Err(_) => {
                                    // Connection is invalid, decrease active count and drop connection
                                    let _ = self.with_internals(|internals| internals.active -= 1);
                                }
                            }
                        }
                        Ok(None) => break, // No more connections
                        Err(_) => break,   // Mutex error, exit loop
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
        // Atomically check connection pool state and perform operations
        let (existing_conn, should_create) = self.with_internals(|internals| {
            // First try to get an existing connection from the pool
            if let Some(conn) = internals.conns.pop_front() {
                return (Some(conn), false);
            }

            // Check if maximum connection limit is exceeded
            let max_size = self.inner.max_size;
            if max_size > 0 && internals.active >= max_size {
                return (None, false); // Exceeds limit, don't create new connection
            }

            // Increase active connection count
            internals.active += 1;
            (None, true) // Need to create new connection
        })?;

        // If there's an existing connection, return it directly
        if let Some(conn) = existing_conn {
            return Ok(Connection {
                conn: Some(conn),
                pool: self.clone(),
            });
        }

        // If shouldn't create new connection (exceeds limit), return error
        if !should_create {
            return Err(std::io::Error::other("exceed limit"));
        }

        // Create new connection
        let conn = self
            .get_timeout(self.inner.connection_timeout)
            .await
            .inspect_err(|_e| {
                // If creation fails, decrease active count
                let _ = self.with_internals(|internals| internals.active -= 1);
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
        // Put connection back to pool, but don't change active count
        // Because connection only changes from "in use" to "idle", total active connections unchanged
        self.push_back(conn)
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
    pub active: u32,
}
