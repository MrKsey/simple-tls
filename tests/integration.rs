use rand::RngCore;
use simple_tls::{cert, client, server};
use std::fs;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const DATA_SIZE: usize = 512 * 1024;
const CONNECTIONS: usize = 16;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raw_and_grpc_tunnels() {
    let _ = env_logger::try_init();
    for grpc in [false, true] {
        for service in ["", "123456"] {
            eprintln!("running grpc={grpc} service={service:?}");
            run_case(grpc, service).await;
        }
    }
}

async fn run_case(grpc: bool, service: &str) {
    let echo_addr = spawn_echo_server().await;

    let generated = cert::generate_cert("", None).expect("generate certificate");
    let dir = std::env::temp_dir().join(format!(
        "simple-tls-test-{}-{}-{}",
        std::process::id(),
        grpc,
        if service.is_empty() { "empty" } else { service }
    ));
    fs::create_dir_all(&dir).unwrap();
    let cert_path = dir.join("test.cert");
    let key_path = dir.join("test.key");
    fs::write(&cert_path, &generated.cert_pem).unwrap();
    fs::write(&key_path, &generated.key_pem).unwrap();

    let server_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_addr = server_listener.local_addr().unwrap();
    let client_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client_addr = client_listener.local_addr().unwrap();

    let server_task = tokio::spawn(server::serve(
        server_listener,
        server::ServerOpts {
            dst: echo_addr.to_string(),
            grpc,
            grpc_path: service.to_owned(),
            cert: cert_path.to_string_lossy().into_owned(),
            key: key_path.to_string_lossy().into_owned(),
            server_name: generated.dns_name.clone(),
            idle_timeout: Duration::from_secs(10),
            outbound_buf: 0,
            inbound_buf: 0,
        },
    ));

    let client_task = tokio::spawn(client::serve(
        client_listener,
        client::ClientOpts {
            dst: server_addr.to_string(),
            grpc,
            grpc_path: service.to_owned(),
            server_name: generated.dns_name.clone(),
            ca: String::new(),
            cert_hash: hex::encode(generated.tbs_sha256),
            no_verify: true,
            idle_timeout: Duration::from_secs(10),
            outbound_buf: 0,
            inbound_buf: 0,
        },
    ));

    let data = Arc::new(random_data(DATA_SIZE));
    let mut tasks = Vec::new();
    for _ in 0..CONNECTIONS {
        let data = data.clone();
        tasks.push(tokio::spawn(async move {
            run_connection(client_addr, data).await;
        }));
    }

    for task in tasks {
        tokio::time::timeout(Duration::from_secs(30), task)
            .await
            .expect("test connection timed out")
            .expect("test connection failed");
    }

    server_task.abort();
    client_task.abort();
    let _ = fs::remove_dir_all(&dir);
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
