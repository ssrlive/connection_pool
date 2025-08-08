use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::Mutex;

// 添加模塊聲明
mod example;
mod improved_pool;

// Define a connection pool that manages a set of TCP connections
struct ConnectionPool {
    connections: VecDeque<TcpStream>,
    max_size: usize,
    address: String,
}

impl ConnectionPool {
    fn new(max_size: usize, address: String) -> Self {
        ConnectionPool {
            connections: VecDeque::new(),
            max_size,
            address,
        }
    }

    async fn get_connection(&mut self) -> io::Result<TcpStream> {
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

    fn release_connection(&mut self, conn: TcpStream) {
        if self.connections.len() < self.max_size {
            self.connections.push_back(conn);
        }
        // If the pool is full, the connection will be dropped (automatically closed)
    }

    async fn is_connection_valid(conn: &TcpStream) -> bool {
        conn.ready(tokio::io::Interest::READABLE | tokio::io::Interest::WRITABLE)
            .await
            .is_ok()
    }
}

// Create a thread-safe connection pool that can be shared across multiple tasks
#[derive(Clone)]
struct SharedConnectionPool {
    pool: Arc<Mutex<ConnectionPool>>,
}

impl SharedConnectionPool {
    fn new(max_size: usize, address: String) -> Self {
        SharedConnectionPool {
            pool: Arc::new(Mutex::new(ConnectionPool::new(max_size, address))),
        }
    }

    async fn get_connection(&self) -> io::Result<TcpStream> {
        self.pool.lock().await.get_connection().await
    }

    async fn release_connection(&self, conn: TcpStream) {
        self.pool.lock().await.release_connection(conn);
    }
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let pool = SharedConnectionPool::new(10, "127.0.0.1:8080".to_string());

    // 模擬多個並發任務使用連接池
    let mut handles = vec![];

    for _ in 0..5 {
        let pool = pool.clone();
        let handle = tokio::spawn(async move {
            let conn = pool.get_connection().await.unwrap();

            // 在這裡使用連接...
            tokio::time::sleep(Duration::from_millis(100)).await;

            pool.release_connection(conn).await;

            Ok::<(), io::Error>(())
        });
        handles.push(handle);
    }

    // 等待所有任務完成
    for handle in handles {
        handle.await.unwrap()?;
    }

    example::run_improved_pool_example().await?;

    Ok(())
}
