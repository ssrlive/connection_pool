use std::io;
use std::time::Duration;

use mypool::mpool::{ManageConnection, Pool};

use async_trait::async_trait;
use tokio::time::sleep;

#[derive(Debug, PartialEq)]
struct FakeConnection;

#[derive(Default)]
struct FakeManager {
    sleep: Option<Duration>,
}

#[async_trait]
impl ManageConnection for FakeManager {
    type Connection = FakeConnection;

    async fn connect(&self) -> io::Result<Self::Connection> {
        if let Some(d) = self.sleep {
            sleep(d).await;
        }

        Ok(FakeConnection)
    }

    async fn check(&self, _conn: &mut Self::Connection) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn test_max_size_ok() {
    let manager = FakeManager::default();
    let pool = Pool::builder().max_size(5).build(manager);
    let mut conns = vec![];
    for _ in 0..5 {
        conns.push(pool.get().await.unwrap());
    }
    assert_eq!(pool.active_count().unwrap(), 5);
    assert!(pool.get().await.is_err());
    assert_eq!(pool.active_count().unwrap(), 5);
}

#[tokio::test]
async fn test_drop_conn() {
    let manager = FakeManager::default();
    let pool = Pool::builder().max_size(2).build(manager);

    let conn1 = pool.get().await.unwrap();
    assert_eq!(pool.active_count().unwrap(), 1);
    let conn2 = pool.get().await.unwrap();
    assert_eq!(pool.active_count().unwrap(), 2);

    // When connections are dropped, they should return to idle state
    drop(conn1);
    assert_eq!(pool.active_count().unwrap(), 1); // conn1 is now idle
    assert_eq!(pool.idle_count().unwrap(), 1);

    drop(conn2);
    assert_eq!(pool.active_count().unwrap(), 0); // both connections are idle
    assert_eq!(pool.idle_count().unwrap(), 2);

    // Get a connection again, should reuse existing idle connection
    let _conn3 = pool.get().await.unwrap();
    assert_eq!(pool.active_count().unwrap(), 1); // one connection active
    assert_eq!(pool.idle_count().unwrap(), 1); // one connection idle
}

#[tokio::test]
async fn test_get_timeout() {
    let manager = FakeManager {
        sleep: Some(Duration::from_millis(200)),
    };

    let pool = Pool::builder()
        .connection_timeout(Some(Duration::from_millis(100)))
        .build(manager);

    assert!(pool.get().await.is_err());
    assert!(
        pool.get_timeout(Some(Duration::from_millis(300)))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn test_idle_timeout() {
    let manager = FakeManager::default();
    let pool = Pool::builder()
        .idle_timeout(Some(Duration::from_secs(1)))
        .check_interval(Some(Duration::from_secs(1)))
        .build(manager);

    let mut conns = vec![];
    for _ in 0..5 {
        conns.push(pool.get().await.unwrap());
    }
    drop(conns);
    sleep(Duration::from_secs(2)).await;
    assert_eq!(pool.active_count().unwrap(), 0);
}

#[tokio::test]
async fn test_max_lifetime() {
    let manager = FakeManager::default();
    let pool = Pool::builder()
        .max_lifetime(Some(Duration::from_secs(1)))
        .check_interval(Some(Duration::from_secs(1)))
        .build(manager);

    let mut conns = vec![];
    for _ in 0..5 {
        conns.push(pool.get().await.unwrap());
    }
    drop(conns);
    sleep(Duration::from_secs(2)).await;
    assert_eq!(pool.active_count().unwrap(), 0);
}
