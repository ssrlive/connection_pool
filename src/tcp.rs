use crate::{CleanupConfig, ConnectionManager, ConnectionPool, PooledStream};
use std::{sync::Arc, time::Duration};
use tokio::net::{TcpStream, ToSocketAddrs};

// Example implementation for TcpStream
pub struct TcpConnectionManager<A: ToSocketAddrs + Send + Sync + Clone + 'static> {
    pub address: A,
}

impl<A> ConnectionManager for TcpConnectionManager<A>
where
    A: ToSocketAddrs + Send + Sync + Clone + 'static,
{
    type Connection = TcpStream;
    type Error = std::io::Error;
    type CreateFut = std::pin::Pin<Box<dyn Future<Output = Result<TcpStream, Self::Error>> + Send>>;
    type ValidFut<'a> = std::pin::Pin<Box<dyn Future<Output = bool> + Send + 'a>>;

    fn create_connection(&self) -> Self::CreateFut {
        let addr = self.address.clone();
        Box::pin(async move { TcpStream::connect(addr).await })
    }

    fn is_valid<'a>(&'a self, stream: &'a Self::Connection) -> Self::ValidFut<'a> {
        Box::pin(async move {
            stream
                .ready(tokio::io::Interest::READABLE | tokio::io::Interest::WRITABLE)
                .await
                .is_ok()
        })
    }
}

// Convenience type aliases
pub type TcpConnectionPool<A = std::net::SocketAddr> = ConnectionPool<TcpConnectionManager<A>>;
pub type TcpPooledStream<A = std::net::SocketAddr> = PooledStream<TcpConnectionManager<A>>;

impl<A> TcpConnectionPool<A>
where
    A: ToSocketAddrs + Send + Sync + Clone + 'static,
{
    pub fn new_tcp(
        max_size: Option<usize>,
        max_idle_time: Option<Duration>,
        connection_timeout: Option<Duration>,
        cleanup_config: Option<CleanupConfig>,
        address: A,
    ) -> Arc<Self> {
        log::info!("Creating TCP connection pool");
        let manager = TcpConnectionManager { address };
        Self::new(max_size, max_idle_time, connection_timeout, cleanup_config, manager)
    }
}
