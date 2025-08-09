use connection_pool::TcpConnectionPool;
use std::net::SocketAddr;
use std::time::Duration;

#[cfg(test)]
#[tokio::test]
async fn test_to_socket_addrs_support() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    println!("Testing ToSocketAddrs support");

    // Test 1: Using String (backwards compatibility)
    println!("\n1. Using String address:");
    let _pool1 = TcpConnectionPool::new_tcp(
        Some(5),
        Some(Duration::from_secs(300)),
        Some(Duration::from_secs(10)),
        None,
        "127.0.0.1:8080".to_string(),
    );
    println!("Created pool with String address: 127.0.0.1:8080");

    // Test 2: Using &str
    println!("\n2. Using &str address:");
    let _pool2 = TcpConnectionPool::new_tcp(
        Some(5),
        Some(Duration::from_secs(300)),
        Some(Duration::from_secs(10)),
        None,
        "127.0.0.1:8081",
    );
    println!("Created pool with &str address: 127.0.0.1:8081");

    // Test 3: Using SocketAddr
    println!("\n3. Using SocketAddr:");
    let addr: SocketAddr = "127.0.0.1:8082".parse()?;
    let _pool3 = TcpConnectionPool::new_tcp(Some(5), Some(Duration::from_secs(300)), Some(Duration::from_secs(10)), None, addr);
    println!("Created pool with SocketAddr: {}", addr);

    // Test 4: Using tuple (Ipv4Addr, u16)
    println!("\n4. Using (Ipv4Addr, u16) tuple:");
    use std::net::Ipv4Addr;
    let _pool4 = TcpConnectionPool::new_tcp(
        Some(5),
        Some(Duration::from_secs(300)),
        Some(Duration::from_secs(10)),
        None,
        (Ipv4Addr::new(127, 0, 0, 1), 8083),
    );
    println!("Created pool with (Ipv4Addr, u16): (127.0.0.1, 8083)");

    // Test 5: Using (&str, u16) tuple
    println!("\n5. Using (&str, u16) tuple:");
    let _pool5 = TcpConnectionPool::new_tcp(
        Some(5),
        Some(Duration::from_secs(300)),
        Some(Duration::from_secs(10)),
        None,
        ("localhost", 8084),
    );
    println!("Created pool with (&str, u16): (localhost, 8084)");

    println!("\nAll address types work! The API now supports any type implementing ToSocketAddrs.");
    println!("This includes: String, &str, SocketAddr, (IpAddr, u16), (&str, u16), and more.");

    Ok(())
}
