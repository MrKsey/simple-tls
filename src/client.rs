use anyhow::{Context, Result};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_rustls::TlsConnector;

use crate::grpc::GrpcPool;
use crate::net;
use crate::tls;
use crate::tunnel;

pub struct ClientOpts {
    pub dst: String,
    pub grpc: bool,
    pub grpc_path: String,
    pub server_name: String,
    pub ca: String,
    pub cert_hash: String,
    pub no_verify: bool,
    pub idle_timeout: Duration,
    pub outbound_buf: i32,
    pub inbound_buf: i32,
}

pub async fn serve(listener: TcpListener, opts: ClientOpts) -> Result<()> {
    let server_name = if opts.server_name.is_empty() {
        net::default_server_name(&opts.dst)
    } else {
        opts.server_name.clone()
    };
    let server_name = tls::parse_server_name(&server_name)?;
    let tls_config =
        tls::build_client_config(&opts.ca, &opts.cert_hash, opts.no_verify, opts.grpc)?;

    if opts.grpc {
        let pool = GrpcPool::new(
            opts.dst.clone(),
            tls_config,
            server_name,
            opts.grpc_path.clone(),
            opts.outbound_buf,
        );

        loop {
            let (local, peer) = listener.accept().await?;
            net::apply_socket_buf(&local, opts.inbound_buf);
            let pool = pool.clone();
            let idle_timeout = opts.idle_timeout;

            tokio::spawn(async move {
                match pool.connect_stream().await {
                    Ok(remote) => {
                        if let Err(error) =
                            tunnel::tunnel(local, remote, idle_timeout, tunnel::GRPC_BUF_SIZE).await
                        {
                            log::debug!("tunnel {peer} closed: {error}");
                        }
                    }
                    Err(error) => {
                        log::warn!("failed to dial server connection for {peer}: {error:#}");
                    }
                }
            });
        }
    } else {
        let connector = TlsConnector::from(tls_config);

        loop {
            let (local, peer) = listener.accept().await?;
            net::apply_socket_buf(&local, opts.inbound_buf);
            let connector = connector.clone();
            let server_name = server_name.clone();
            let dst = opts.dst.clone();
            let outbound_buf = opts.outbound_buf;
            let idle_timeout = opts.idle_timeout;

            tokio::spawn(async move {
                let result = async {
                    let tcp = net::connect(&dst, outbound_buf).await?;
                    let tls = tokio::time::timeout(
                        Duration::from_secs(5),
                        connector.connect(server_name, tcp),
                    )
                    .await
                    .map_err(|_| {
                        std::io::Error::new(std::io::ErrorKind::TimedOut, "TLS handshake timeout")
                    })??;
                    tunnel::tunnel(local, tls, idle_timeout, tunnel::RAW_BUF_SIZE).await
                }
                .await;

                if let Err(error) = result {
                    log::debug!("tunnel {peer} closed: {error}");
                }
            });
        }
    }
}

pub async fn bind(addr: &str) -> Result<TcpListener> {
    net::bind_listener(addr)
        .await
        .with_context(|| format!("cannot bind {addr}"))
}
