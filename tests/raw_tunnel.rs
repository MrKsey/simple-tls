use rand::RngCore;
use simple_tls::tunnel::tunnel;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const DATA_SIZE: usize = 512 * 1024;
const CONNECTIONS: usize = 16;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raw_tcp_tunnel_512k() {
    let _ = env_logger::try_init();
    let echo_addr = spawn_echo_server().await;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let echo_addr = echo_addr;
            tokio::spawn(async move {
                let Ok(remote) = TcpStream::connect(echo_addr).await else {
                    return;
                };
                let _ = tunnel(stream, remote, Duration::from_secs(10), 64 * 1024).await;
            });
        }
    });

    let data = Arc::new(random_data(DATA_SIZE));
    let mut tasks = Vec::new();
    for _ in 0..CONNECTIONS {
        let data = data.clone();
        tasks.push(tokio::spawn(async move {
            run_connection(addr, data).await;
        }));
    }

    for task in tasks {
        tokio::time::timeout(Duration::from_secs(30), task)
            .await
            .expect("test connection timed out")
            .expect("test connection failed");
    }
}

async fn run_connection(addr: SocketAddr, data: Arc<Vec<u8>>) {
    let stream = TcpStream::connect(addr).await.unwrap();
    stream.set_nodelay(true).unwrap();
    let (mut reader, mut writer) = stream.into_split();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

    let write_data = data.clone();
    let write_task = tokio::spawn(async move {
        writer.write_all(&write_data).await.unwrap();
        let _ = done_rx.await;
    });

    let mut received = vec![0u8; data.len()];
    reader.read_exact(&mut received).await.unwrap();
    assert_eq!(&received[..], &data[..], "corrupted data");

    let _ = done_tx.send(());
    write_task.await.unwrap();
}

async fn spawn_echo_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                loop {
                    let Ok(n) = stream.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    if stream.write_all(&buf[..n]).await.is_err() {
                        return;
                    }
                }
            });
        }
    });

    addr
}

fn random_data(size: usize) -> Vec<u8> {
    let mut data = vec![0u8; size];
    rand::thread_rng().fill_bytes(&mut data);
    data
}
