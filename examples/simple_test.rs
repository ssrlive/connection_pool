//! Before running this example, make sure you have a TCP server running on 127.0.0.1:8080
//! You can install from cargo and run it to create a simple TCP server for testing:
//! ```bash
//! cargo install socks5-impl --git https://github.com/tun2proxy/socks5-impl --example echo-server
//! echo-server -l 127.0.0.1:8080
//! ```

use mypool::simple_pool::SharedConnectionPool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let pool = SharedConnectionPool::new(5, "127.0.0.1:8080".to_string());

    // Simulate multiple concurrent tasks using the connection pool
    let mut handles = vec![];

    for i in 0..20 {
        let pool = pool.clone();
        let handle = tokio::spawn(async move {
            let mut conn = pool.get_connection().await.unwrap();

            conn.write(b"Hello, world!").await?;
            conn.flush().await?;
            let mut buf = vec![0; 1024];
            conn.read_buf(&mut buf).await?;
            println!("Task {i} received: {}", String::from_utf8_lossy(&buf));

            pool.release_connection(conn).await;

            Ok::<(), std::io::Error>(())
        });
        handles.push(handle);
    }

    // Wait for all tasks to complete
    for handle in handles {
        handle.await.unwrap()?;
    }

    Ok(())
}
