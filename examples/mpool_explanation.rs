use async_trait::async_trait;
use mypool::mpool::{ManageConnection, Pool};
use std::io;
use std::time::Duration;

// 1. Define your connection type
#[derive(Debug)]
struct MyConnection {
    id: u32,
    is_connected: bool,
}

// 2. Define connection manager
struct MyConnectionManager {
    next_id: std::sync::atomic::AtomicU32,
}

impl MyConnectionManager {
    fn new() -> Self {
        Self {
            next_id: std::sync::atomic::AtomicU32::new(1),
        }
    }
}

// 3. Implement ManageConnection trait
#[async_trait]
impl ManageConnection for MyConnectionManager {
    type Connection = MyConnection;

    async fn connect(&self) -> io::Result<Self::Connection> {
        // Simulate connection creation process
        println!("Creating new connection...");
        tokio::time::sleep(Duration::from_millis(100)).await;

        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(MyConnection {
            id,
            is_connected: true,
        })
    }

    async fn check(&self, conn: &mut Self::Connection) -> io::Result<()> {
        // Simulate health check
        if conn.is_connected {
            println!("Connection {} is healthy", conn.id);
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "Connection lost"))
        }
    }
}

#[tokio::main]
async fn main() -> io::Result<()> {
    println!("🚀 mpool Principle Demonstration");

    // 4. Create connection pool
    let manager = MyConnectionManager::new();
    let pool = Pool::builder()
        .max_size(3) // Maximum 3 connections
        .idle_timeout(Some(Duration::from_secs(5))) // 5 second idle timeout
        .max_lifetime(Some(Duration::from_secs(30))) // 30 second maximum lifetime
        .connection_timeout(Some(Duration::from_secs(2))) // 2 second creation timeout
        .check_interval(Some(Duration::from_secs(2))) // 2 second check interval
        .build(manager);

    println!("\n📊 Initial pool state:");
    let stats = pool.stats()?;
    println!(
        "Active connections: {}, Idle connections: {}, Max limit: {}",
        stats.active, stats.idle, stats.max_size
    );

    // 5. Get and use connections
    println!("\n🔗 Getting first connection:");
    let conn1 = pool.get().await?;
    println!("Got connection ID: {}", conn1.id);

    let stats = pool.stats()?;
    println!(
        "📊 Pool state: Active={}, Idle={}",
        stats.active, stats.idle
    );

    println!("\n🔗 Getting second connection:");
    let conn2 = pool.get().await?;
    println!("Got connection ID: {}", conn2.id);

    let stats = pool.stats()?;
    println!(
        "📊 Pool state: Active={}, Idle={}",
        stats.active, stats.idle
    );

    println!("\n🔗 Getting third connection:");
    let conn3 = pool.get().await?;
    println!("Got connection ID: {}", conn3.id);

    let stats = pool.stats()?;
    println!(
        "📊 Pool state: Active={}, Idle={}",
        stats.active, stats.idle
    );

    // 6. Try to get fourth connection (should fail)
    println!("\n❌ Attempting to get fourth connection (exceeds limit):");
    match pool.get().await {
        Ok(_) => println!("Unexpected connection obtained!"),
        Err(e) => println!("Expected error: {e}"),
    }

    // 7. Release a connection
    println!("\n🔄 Releasing first connection:");
    drop(conn1); // Connection automatically returned to pool

    let stats = pool.stats()?;
    println!(
        "📊 Pool state: Active={}, Idle={}",
        stats.active, stats.idle
    );

    // 8. Now can get connection again
    println!("\n♻️  Re-acquiring connection (reuse):");
    let conn4 = pool.get().await?;
    println!(
        "Got connection ID: {} (this is a reused connection)",
        conn4.id
    );

    let stats = pool.stats()?;
    println!(
        "📊 Final pool state: Active={}, Idle={}",
        stats.active, stats.idle
    );

    // 9. Wait for background health check to run
    println!("\n⏰ Waiting for background health check...");
    tokio::time::sleep(Duration::from_secs(3)).await;

    Ok(())
}
