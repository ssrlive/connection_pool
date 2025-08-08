use crate::improved_pool::ConnectionPool;
use std::io;
use std::time::Duration;

pub async fn run_improved_pool_example() -> io::Result<()> {
    let pool = ConnectionPool::new(5, "127.0.0.1:8080".to_string());

    // 模擬多個並發任務使用改進的連接池
    let mut handles = vec![];

    for i in 0..10 {
        let pool = pool.clone();
        let handle = tokio::spawn(async move {
            println!("Task {i} trying to get connection");

            let _conn = pool.get_connection().await?;

            println!("Task {i} got connection");

            // 模擬使用連接
            tokio::time::sleep(Duration::from_millis(100)).await;

            println!("Task {i} finished using connection");
            // 連接會在 conn 被 drop 時自動歸還到池中

            Ok::<(), io::Error>(())
        });
        handles.push(handle);
    }

    // 等待所有任務完成
    for handle in handles {
        handle.await.unwrap()?;
    }

    println!("All tasks completed");
    Ok(())
}
