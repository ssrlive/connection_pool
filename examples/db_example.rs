use connection_pool::generic_pool::{ConnectionCreator, ConnectionPool, ConnectionValidator};
use std::future::Future;

// Custom connection type - simulating database connection
#[derive(Debug)]
pub struct DatabaseConnection {
    pub id: u32,
    pub connected: bool,
}

impl DatabaseConnection {
    fn new(id: u32) -> Self {
        Self { id, connected: true }
    }

    fn is_alive(&self) -> bool {
        self.connected
    }
}

// Database connection parameters
#[derive(Clone)]
pub struct DbConnectionParams {
    pub host: String,
    pub port: u16,
    pub database: String,
}

// Database connection creator
pub struct DbConnectionCreator;

impl ConnectionCreator<DatabaseConnection, DbConnectionParams> for DbConnectionCreator {
    type Error = String;
    type Future = std::pin::Pin<Box<dyn Future<Output = Result<DatabaseConnection, Self::Error>> + Send>>;

    fn create_connection(&self, params: &DbConnectionParams) -> Self::Future {
        let _params = params.clone();
        Box::pin(async move {
            // Simulate database connection process
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

            // Use a simple counter
            static mut COUNTER: u32 = 0;
            unsafe {
                COUNTER += 1;
                Ok(DatabaseConnection::new(COUNTER))
            }
        })
    }
}

// Database connection validator
pub struct DbConnectionValidator;

impl ConnectionValidator<DatabaseConnection> for DbConnectionValidator {
    async fn is_valid(&self, connection: &DatabaseConnection) -> bool {
        connection.is_alive()
    }
}

// Database connection pool type alias
pub type DbConnectionPool = ConnectionPool<DatabaseConnection, DbConnectionParams, DbConnectionCreator, DbConnectionValidator>;

pub async fn example_db_pool() -> Result<(), Box<dyn std::error::Error>> {
    let params = DbConnectionParams {
        host: "localhost".to_string(),
        port: 5432,
        database: "myapp".to_string(),
    };

    println!("Creating database connection pool...");

    // Create database connection pool
    let pool = DbConnectionPool::new(
        Some(3),               // Maximum number of connections
        None,                  // Use default idle timeout
        None,                  // Use default connection timeout
        None,                  // Use default cleanup config (enabled with 30s interval)
        params,                // Connection parameters
        DbConnectionCreator,   // Connection creator
        DbConnectionValidator, // Connection validator
    );

    println!("Testing concurrent connection acquisition...");

    // Test concurrent acquisition of multiple connections
    let mut handles = vec![];

    for i in 1..=5 {
        let pool_clone = pool.clone();
        let handle = tokio::spawn(async move {
            match pool_clone.get_connection().await {
                Ok(conn) => {
                    println!("Task {i} successfully acquired database connection ID: {}", conn.id);
                    // Simulate using the connection
                    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                    println!("Task {i} completed, returning connection ID: {}", conn.id);
                }
                Err(e) => {
                    println!("Task {i} failed to acquire database connection: {e}");
                }
            }
        });
        handles.push(handle);
    }

    // Wait for all tasks to complete
    for handle in handles {
        handle.await?;
    }

    println!("Database connection pool example completed!");

    Ok(())
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    env_logger::Builder::from_default_env().filter_level(log::LevelFilter::Trace).init();

    println!("=== Connection Pool Example ===");

    println!("=== Running Generic Connection Pool Example ===");
    if let Err(e) = example_db_pool().await {
        println!("Database connection pool example error: {e}");
    }

    Ok(())
}
