use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, Semaphore};
use tokio::time::timeout;

pub struct ConnectionPool {
    connections: Arc<Mutex<VecDeque<PooledConnection>>>,
    semaphore: Arc<Semaphore>,
    max_size: usize,
    address: String,
    max_idle_time: Duration,
}

struct PooledConnection {
    stream: TcpStream,
    created_at: Instant,
}

pub struct PooledStream {
    stream: Option<TcpStream>,
    pool: Arc<ConnectionPool>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl ConnectionPool {
    pub fn new(max_size: usize, address: String) -> Arc<Self> {
        Arc::new(ConnectionPool {
            connections: Arc::new(Mutex::new(VecDeque::new())),
            semaphore: Arc::new(Semaphore::new(max_size)),
            max_size,
            address,
            max_idle_time: Duration::from_secs(300), // 5 minutes idle timeout
        })
    }

    pub async fn get_connection(self: Arc<Self>) -> io::Result<PooledStream> {
        // Use semaphore to limit concurrent connections
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| io::Error::other("Pool is closed"))?;

        {
            // Try to get an existing connection from the pool
            let mut connections = self.connections.lock().await;

            self.cleanup_expired_connections(&mut connections).await;

            if let Some(pooled_conn) = connections.pop_front() {
                if self.is_connection_valid(&pooled_conn.stream).await {
                    return Ok(PooledStream {
                        stream: Some(pooled_conn.stream),
                        pool: self.clone(),
                        _permit: permit,
                    });
                }
            }
            // drop(connections); // Release lock
        }

        // Create new connection
        match timeout(Duration::from_secs(10), TcpStream::connect(&self.address)).await {
            Ok(Ok(stream)) => Ok(PooledStream {
                stream: Some(stream),
                pool: self.clone(),
                _permit: permit,
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(io::Error::other("Connection timeout")),
        }
    }

    async fn cleanup_expired_connections(&self, connections: &mut VecDeque<PooledConnection>) {
        let now = Instant::now();
        connections.retain(|conn| now.duration_since(conn.created_at) < self.max_idle_time);
    }

    async fn is_connection_valid(&self, stream: &TcpStream) -> bool {
        // More robust check for connection validity
        stream
            .ready(tokio::io::Interest::READABLE | tokio::io::Interest::WRITABLE)
            .await
            .is_ok()
    }

    async fn return_connection(&self, stream: TcpStream) {
        let mut connections = self.connections.lock().await;
        if connections.len() < self.max_size {
            connections.push_back(PooledConnection {
                stream,
                created_at: Instant::now(),
            });
        }
        // If the pool is full, the connection will be dropped (automatically closed)
    }
}

impl AsRef<TcpStream> for PooledStream {
    fn as_ref(&self) -> &TcpStream {
        self.stream.as_ref().unwrap()
    }
}

impl AsMut<TcpStream> for PooledStream {
    fn as_mut(&mut self) -> &mut TcpStream {
        self.stream.as_mut().unwrap()
    }
}

impl Drop for PooledStream {
    fn drop(&mut self) {
        if let Some(stream) = self.stream.take() {
            let pool = self.pool.clone();
            tokio::spawn(async move {
                pool.return_connection(stream).await;
            });
        }
    }
}

// Implement Deref and DerefMut for PooledStream
impl std::ops::Deref for PooledStream {
    type Target = TcpStream;

    fn deref(&self) -> &Self::Target {
        self.stream.as_ref().unwrap()
    }
}

impl std::ops::DerefMut for PooledStream {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.stream.as_mut().unwrap()
    }
}
