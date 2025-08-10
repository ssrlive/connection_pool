use connection_pool::ConnectionPool;
use std::future::Future;
use std::pin::Pin;

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

// Database connection manager
#[derive(Clone)]
pub struct DbConnectionManager {
    pub params: DbConnectionParams,
}

impl DbConnectionManager {
    pub fn new(params: DbConnectionParams) -> Self {
        Self { params }
    }
}

impl connection_pool::ConnectionManager for DbConnectionManager {
    type Connection = DatabaseConnection;
    type Error = std::io::Error;
    type CreateFut = Pin<Box<dyn Future<Output = Result<DatabaseConnection, Self::Error>> + Send>>;
    type ValidFut<'a> = Pin<Box<dyn Future<Output = bool> + Send + 'a>>;

    fn create_connection(&self) -> Self::CreateFut {
        Box::pin(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            static mut COUNTER: u32 = 0;
            unsafe {
                COUNTER += 1;
                Ok(DatabaseConnection::new(COUNTER))
            }
        })
    }

    fn is_valid<'a>(&'a self, connection: &'a Self::Connection) -> Self::ValidFut<'a> {
        Box::pin(async move { connection.is_alive() })
    }
}

// Database connection pool type alias
pub type DbConnectionPool = ConnectionPool<DbConnectionManager>;

pub async fn example_db_pool() -> Result<(), Box<dyn std::error::Error>> {
    let params = DbConnectionParams {
        host: "localhost".to_string(),
        port: 5432,
        database: "myapp".to_string(),
    };

    println!("Creating database connection pool...");

    // Create database connection pool
    let manager = DbConnectionManager::new(params);
    let pool = DbConnectionPool::new(
        Some(3), // Maximum number of connections
        None,    // Use default idle timeout
        None,    // Use default connection timeout
        None,    // Use default cleanup config (enabled with 30s interval)
        manager,
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
