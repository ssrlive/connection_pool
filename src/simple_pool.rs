use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::Mutex;

// Define a connection pool that manages a set of TCP connections
pub struct ConnectionPool {
    connections: VecDeque<TcpStream>,
    max_size: usize,
    address: String,
}

impl ConnectionPool {
    pub fn new(max_size: usize, address: String) -> Self {
        ConnectionPool {
            connections: VecDeque::new(),
            max_size,
            address,
        }
    }

    pub async fn get_connection(&mut self) -> io::Result<TcpStream> {
        if let Some(conn) = self.connections.pop_front() {
            if Self::is_connection_valid(&conn).await {
                return Ok(conn);
            }
        }

        if self.connections.len() < self.max_size {
            Ok(TcpStream::connect(&self.address).await?)
        } else {
            Err(io::Error::other("Connection pool is full"))
        }
    }

    pub fn release_connection(&mut self, conn: TcpStream) {
        if self.connections.len() < self.max_size {
            self.connections.push_back(conn);
        }
        // If the pool is full, the connection will be dropped (automatically closed)
    }

    pub async fn is_connection_valid(conn: &TcpStream) -> bool {
        conn.ready(tokio::io::Interest::READABLE | tokio::io::Interest::WRITABLE)
            .await
            .is_ok()
    }
}

// Create a thread-safe connection pool that can be shared across multiple tasks
#[derive(Clone)]
pub struct SharedConnectionPool {
    pool: Arc<Mutex<ConnectionPool>>,
}

impl SharedConnectionPool {
    pub fn new(max_size: usize, address: String) -> Self {
        SharedConnectionPool {
            pool: Arc::new(Mutex::new(ConnectionPool::new(max_size, address))),
        }
    }

    pub async fn get_connection(&self) -> io::Result<TcpStream> {
        self.pool.lock().await.get_connection().await
    }

    pub async fn release_connection(&self, conn: TcpStream) {
        self.pool.lock().await.release_connection(conn);
    }
}
