//! Regression test for grpc-go compatible lazy response headers.
//!
//! grpc-go servers send the response headers only after the first message
//! arrives (or the handler returns). A client that waits for the headers
//! before it starts sending data deadlocks against such servers.

use bytes::Bytes;
use http::{HeaderValue, Response, StatusCode};
use simple_tls::{cert, client, tls};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const DATA_SIZE: usize = 64 * 1024;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_works_with_lazy_grpc_headers() {
    let _ = env_logger::try_init();

    let generated = cert::generate_cert("lazy.test", None).expect("generate certificate");
    let server_config = tls::build_server_config(
        generated.cert_pem.as_bytes(),
        generated.key_pem.as_bytes(),
        true,
    )
    .expect("server tls config");

    let lazy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let lazy_addr = lazy_listener.local_addr().unwrap();
    tokio::spawn(run_lazy_server(lazy_listener, server_config));

    let client_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client_addr = client_listener.local_addr().unwrap();
    let client_task = tokio::spawn(client::serve(
        client_listener,
        client::ClientOpts {
            dst: lazy_addr.to_string(),
            grpc: true,
            grpc_path: "backup".to_owned(),
            server_name: "lazy.test".to_owned(),
            ca: String::new(),
            cert_hash: hex::encode(generated.tbs_sha256),
            no_verify: true,
            idle_timeout: Duration::from_secs(10),
            outbound_buf: 0,
            inbound_buf: 0,
        },
    ));

    let result = tokio::time::timeout(Duration::from_secs(15), async {
        let stream = TcpStream::connect(client_addr).await.unwrap();
        stream.set_nodelay(true).unwrap();
        let (mut reader, mut writer) = stream.into_split();
        let data = vec![0x5au8; DATA_SIZE];

        let write_task = tokio::spawn(async move {
            writer.write_all(&data).await.unwrap();
            writer
        });

        let mut received = vec![0u8; DATA_SIZE];
        reader.read_exact(&mut received).await.unwrap();
        assert!(received.iter().all(|byte| *byte == 0x5a));
        let mut writer = write_task.await.unwrap();
        let _ = writer.shutdown().await;
    })
    .await;

    client_task.abort();
    result.expect("grpc tunnel with lazy response headers timed out");
}

async fn run_lazy_server(listener: TcpListener, config: std::sync::Arc<rustls::ServerConfig>) {
    let acceptor = tokio_rustls::TlsAcceptor::from(config);
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            return;
        };
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            let Ok(tls) = acceptor.accept(tcp).await else {
                return;
            };
            let Ok(mut connection) = h2::server::handshake(tls).await else {
                return;
            };
            while let Some(Ok((request, respond))) = connection.accept().await {
                tokio::spawn(handle_lazy_request(request, respond));
            }
        });
    }
}

async fn handle_lazy_request(
    request: http::Request<h2::RecvStream>,
    mut respond: h2::server::SendResponse<Bytes>,
) {
    assert_eq!(request.uri().path(), "/backup/Connect");
    let mut recv = request.into_body();

    // Wait for the first message before sending any response headers. This is
    // what grpc-go does, and what used to deadlock our client.
    let first = recv.data().await;

    let mut response = Response::new(());
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/grpc"),
    );
    let Ok(mut send) = respond.send_response(response, false) else {
        return;
    };

    if let Some(Ok(chunk)) = first {
        let size = chunk.len();
        if send.send_data(chunk, false).is_err() {
            return;
        }
        if recv.flow_control().release_capacity(size).is_err() {
            return;
        }
    }
    while let Some(chunk) = recv.data().await {
        let Ok(chunk) = chunk else {
            return;
        };
        let size = chunk.len();
        if send.send_data(chunk, false).is_err() {
            return;
        }
        if recv.flow_control().release_capacity(size).is_err() {
            return;
        }
    }

    let mut trailers = http::HeaderMap::new();
    trailers.insert("grpc-status", HeaderValue::from_static("0"));
    let _ = send.send_trailers(trailers);
}
