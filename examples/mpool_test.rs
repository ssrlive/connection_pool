use connection_pool::mpool::{ManageConnection, Pool};
use std::io;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

struct MyPool {
    addr: SocketAddr,
}

#[async_trait::async_trait]
impl ManageConnection for MyPool {
    type Connection = TcpStream;

    async fn connect(&self) -> io::Result<Self::Connection> {
        TcpStream::connect(self.addr).await
    }

    async fn check(&self, conn: &mut Self::Connection) -> io::Result<()> {
        // Perform a simple health check by trying to write/read
        match tokio::time::timeout(Duration::from_secs(1), async {
            conn.ready(tokio::io::Interest::READABLE | tokio::io::Interest::WRITABLE).await
        })
        .await
        {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, "Health check timeout")),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 Starting mpool TCP connection test");

    let manager = MyPool {
        addr: "127.0.0.1:8080".parse().unwrap(),
    };

    let pool = Pool::builder()
        .max_size(5)
        .idle_timeout(Some(Duration::from_secs(30)))
        .max_lifetime(Some(Duration::from_secs(300)))
        .connection_timeout(Some(Duration::from_secs(5)))
        .check_interval(Some(Duration::from_secs(10)))
        .build(manager);

    println!("📊 Pool created with max_size=5");

    // Test if server is available
    match test_server_connection(&pool).await {
        Ok(_) => println!("✅ Server connection test passed"),
        Err(e) => {
            println!("❌ Server connection failed: {e}");
            println!("💡 Hint: Start a TCP echo server with: nc -l 127.0.0.1 8080");
            println!("   Or use any echo server on port 8080");
            return Ok(());
        }
    }

    // Run multiple concurrent tasks
    println!("\n🔄 Running concurrent connection tests...");

    let mut handles = vec![];

    for task_id in 0..15 {
        // More than pool size to test limiting
        let pool_clone = pool.clone();
        let handle = tokio::spawn(async move {
            match perform_echo_test(&pool_clone, task_id).await {
                Ok(response) => {
                    println!("✅ Task {task_id}: Echo successful - '{response}'");
                    Ok(())
                }
                Err(e) => {
                    println!("❌ Task {task_id}: Echo failed - {e}");
                    Err(e)
                }
            }
        });
        handles.push(handle);

        // Small delay between task starts to see connection reuse
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Wait for all tasks and collect results
    let mut success_count = 0;
    let mut error_count = 0;

    for handle in handles {
        match handle.await {
            Ok(Ok(())) => success_count += 1,
            Ok(Err(_)) => error_count += 1,
            Err(e) => {
                println!("⚠️  Task join error: {e}");
                error_count += 1;
            }
        }
    }

    println!("\n📈 Test Results Summary:");
    println!("   ✅ Successful operations: {success_count}");
    println!("   ❌ Failed operations: {error_count}");
    println!(
        "   📊 Success rate: {:.1}%",
        (success_count as f32 / (success_count + error_count) as f32) * 100.0
    );

    // Performance test
    println!("\n⚡ Running performance test...");
    let start_time = std::time::Instant::now();

    let mut perf_handles = vec![];
    for i in 0..20 {
        let pool_clone = pool.clone();
        let handle = tokio::spawn(async move {
            for j in 0..5 {
                let message_id = i * 5 + j;
                if let Err(e) = perform_simple_echo(&pool_clone, message_id).await {
                    println!("⚠️  Performance test {i}-{j} failed: {e}");
                }
            }
        });
        perf_handles.push(handle);
    }

    for handle in perf_handles {
        handle.await?;
    }

    let elapsed = start_time.elapsed();
    println!("   ⏱️  Completed 100 operations in {:.2}s", elapsed.as_secs_f64());
    println!("   📊 Average: {:.2} ops/sec", 100.0 / elapsed.as_secs_f64());
    println!("\n📊 Final pool statistics:");
    let stats = pool.stats()?;
    println!("   Active connections: {}", stats.active);
    println!("   Idle connections: {}", stats.idle);
    println!("   Max pool size: {}", stats.max_size);

    println!("\n✨ mpool test completed successfully!");

    Ok(())
}

async fn test_server_connection(pool: &Pool<MyPool>) -> io::Result<()> {
    let mut conn = pool.get().await?;

    // Try to write a simple test message
    conn.write_all(b"test\n").await?;
    conn.flush().await?;

    // Try to read response (with timeout)
    let mut buffer = [0u8; 1024];
    let bytes_read = tokio::time::timeout(Duration::from_secs(2), conn.read(&mut buffer))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Read timeout"))??;

    let response = String::from_utf8_lossy(&buffer[..bytes_read]);
    println!("Task test_server_connection received: {response}");

    Ok(())
}

async fn perform_echo_test(pool: &Pool<MyPool>, task_id: usize) -> io::Result<String> {
    // Get connection from pool
    let mut conn = pool.get().await?;

    // Prepare message
    let message = format!("Hello from task {task_id}\n");

    // Write message
    conn.write_all(message.as_bytes()).await?;
    conn.flush().await?;

    // Read response
    let mut buffer = [0u8; 1024];
    let bytes_read = tokio::time::timeout(Duration::from_secs(3), conn.read(&mut buffer))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Read timeout"))??;

    if bytes_read == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Connection closed"));
    }

    let response = String::from_utf8_lossy(&buffer[..bytes_read]);
    let response = response.trim().to_string();

    // Small delay to simulate processing
    tokio::time::sleep(Duration::from_millis(100)).await;

    Ok(response)
}

async fn perform_simple_echo(pool: &Pool<MyPool>, message_id: usize) -> io::Result<()> {
    // Get connection from pool
    let mut conn = pool.get().await?;

    // Prepare simple message
    let message = format!("msg{message_id}\n");

    // Write and read with shorter timeout for performance test
    conn.write_all(message.as_bytes()).await?;
    conn.flush().await?;

    let mut buffer = [0u8; 64];
    let _bytes_read = tokio::time::timeout(Duration::from_millis(500), conn.read(&mut buffer))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Read timeout"))??;

    Ok(())
}
