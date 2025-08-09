use connection_pool::generic_pool::{CleanupConfig, TcpConnectionPool};
use std::time::Duration;
use tokio::time::sleep;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logger
    env_logger::Builder::from_default_env().filter_level(log::LevelFilter::Debug).init();

    println!("=== Background Cleanup Example ===");

    // Create a connection pool with background cleanup enabled
    let cleanup_config = CleanupConfig {
        interval: Duration::from_secs(5), // Clean up every 5 seconds
        enabled: true,
    };

    let pool = TcpConnectionPool::new_tcp(
        Some(3),                       // max 3 connections
        Some(Duration::from_secs(10)), // idle timeout: 10 seconds
        Some(Duration::from_secs(5)),  // connection timeout: 5 seconds
        Some(cleanup_config),
        "127.0.0.1:8080".to_string(),
    );

    println!("Created connection pool with background cleanup");
    println!("- Max connections: 3");
    println!("- Idle timeout: 10 seconds");
    println!("- Cleanup interval: 5 seconds");

    // Note: This example will try to connect to a server that might not exist
    // The point is to demonstrate the cleanup mechanism, not successful connections

    println!("\nAttempting to create some connections...");

    // Try to create connections (they might fail, but that's ok for this demo)
    for i in 1..=5 {
        match pool.clone().get_connection().await {
            Ok(conn) => {
                println!("Connection {} created successfully", i);
                // Hold the connection briefly, then drop it
                sleep(Duration::from_millis(100)).await;
                drop(conn);
                println!("Connection {} dropped", i);
            }
            Err(e) => {
                println!("Connection {} failed: {}", i, e);
            }
        }
        sleep(Duration::from_millis(500)).await;
    }

    println!("\nWaiting for background cleanup to run...");
    sleep(Duration::from_secs(15)).await;

    println!("\nManually stopping cleanup task...");
    pool.stop_cleanup_task().await;

    println!("\nRestarting cleanup with different interval...");
    let new_config = CleanupConfig {
        interval: Duration::from_secs(2), // Faster cleanup
        enabled: true,
    };
    pool.restart_cleanup_task(new_config).await;

    println!("Waiting a bit more to see the new cleanup interval...");
    sleep(Duration::from_secs(10)).await;

    println!("\nBackground cleanup example completed!");
    Ok(())
}
