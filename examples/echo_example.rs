//! Before running this example, make sure you have a TCP server running on 127.0.0.1:8080
//! You can install from cargo and run it to create a simple TCP server for testing:
//! ```bash
//! cargo install socks5-impl --git https://github.com/tun2proxy/socks5-impl --example echo-server
//! echo-server -l 127.0.0.1:8080
//! ```

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use connection_pool::generic_pool::TcpConnectionPool;

pub async fn run_generic_pool_example() -> std::io::Result<()> {
    let pool = TcpConnectionPool::new_tcp(Some(5), None, None, None, "127.0.0.1:8080".to_string());

    // Simulate multiple concurrent tasks using the generic connection pool
    let mut handles = vec![];

    for i in 0..10 {
        let pool = pool.clone();
        let handle = tokio::spawn(async move {
            println!("Task {i} trying to get connection");

            let mut conn = pool
                .get_connection()
                .await
                .map_err(|e| std::io::Error::other(format!("Task {i} failed to get connection: {e}")))?;

            println!("Task {i} got connection");

            _ = conn.write(b"Hello, world!").await?;
            conn.flush().await?;
            let mut buf = vec![0; 1024];
            conn.read_buf(&mut buf).await?;
            println!("Task {i} received: {}", String::from_utf8_lossy(&buf));

            println!("Task {i} finished using connection");
            // Connection will be automatically returned to the pool when conn is dropped

            Ok::<(), std::io::Error>(())
        });
        handles.push(handle);
    }

    // Wait for all tasks to complete
    for handle in handles {
        handle.await.unwrap()?;
    }

    println!("All tasks completed");
    Ok(())
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    env_logger::Builder::from_default_env().filter_level(log::LevelFilter::Trace).init();
    println!("=== Running Echo Connection Pool Example ===");
    if let Err(e) = run_generic_pool_example().await {
        println!("Echo connection pool example error: {e}");
    }
    Ok(())
}
