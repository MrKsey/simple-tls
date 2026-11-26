use anyhow::{bail, Context, Result};
use std::fs;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use crate::cert;
use crate::grpc;
use crate::net;
use crate::tls;
use crate::tunnel;

pub struct ServerOpts {
    pub dst: String,
    pub grpc: bool,
    pub grpc_path: String,
    pub cert: String,
    pub key: String,
    pub server_name: String,
    pub idle_timeout: Duration,
    pub outbound_buf: i32,
    pub inbound_buf: i32,
}

pub async fn serve(listener: TcpListener, opts: ServerOpts) -> Result<()> {
    let (cert_pem, key_pem) = load_server_cert(&opts)?;
    let tls_config = tls::build_server_config(&cert_pem, &key_pem, opts.grpc)?;
    let acceptor = TlsAcceptor::from(tls_config);

    if opts.grpc {
        let routes = Arc::new(grpc::build_routes(&opts.dst, &opts.grpc_path)?);
        loop {
            let (tcp, peer) = net::accept(&listener).await?;
            net::apply_socket_buf(&tcp, opts.inbound_buf);
            let acceptor = acceptor.clone();
            let routes = routes.clone();
            let idle_timeout = opts.idle_timeout;
            let outbound_buf = opts.outbound_buf;

            tokio::spawn(async move {
                let result = async {
                    let tls = tokio::time::timeout(Duration::from_secs(3), acceptor.accept(tcp))
                        .await
                        .map_err(|_| {
                            std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "TLS handshake timeout",
                            )
                        })??;
                    grpc::serve_connection(tls, routes, idle_timeout, outbound_buf).await
                }
                .await;

                if let Err(error) = result {
                    log::debug!("grpc connection {peer} closed: {error:#}");
                }
            });
        }
    } else {
        loop {
            let (tcp, peer) = net::accept(&listener).await?;
            net::apply_socket_buf(&tcp, opts.inbound_buf);
            let acceptor = acceptor.clone();
            let dst = opts.dst.clone();
            let idle_timeout = opts.idle_timeout;
            let outbound_buf = opts.outbound_buf;

            tokio::spawn(async move {
                let result = async {
                    let tls = tokio::time::timeout(Duration::from_secs(3), acceptor.accept(tcp))
                        .await
                        .map_err(|_| {
                            std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "TLS handshake timeout",
                            )
                        })??;
                    let dst_conn = net::connect(&dst, outbound_buf).await?;
                    tunnel::tunnel(dst_conn, tls, idle_timeout, tunnel::RAW_BUF_SIZE).await
                }
                .await;

                if let Err(error) = result {
                    log::debug!("tunnel {peer} closed: {error}");
                }
            });
        }
    }
}

fn load_server_cert(opts: &ServerOpts) -> Result<(Vec<u8>, Vec<u8>)> {
    let env_cert = std::env::var("SIMPLE_TLS_CERT").unwrap_or_default();
    let env_key = std::env::var("SIMPLE_TLS_KEY").unwrap_or_default();

    if !env_cert.is_empty() && !env_key.is_empty() {
        return Ok((env_cert.into_bytes(), env_key.into_bytes()));
    }

    match (opts.cert.is_empty(), opts.key.is_empty()) {
        (true, true) => {
            let generated = cert::generate_cert(&opts.server_name, None)
                .context("failed to generate temp cert")?;
            log::warn!(
                "warning: you are using a tmp certificate with dns name: {}",
                generated.dns_name
            );
            Ok((
                generated.cert_pem.into_bytes(),
                generated.key_pem.into_bytes(),
            ))
        }
        (false, false) => {
            let cert_pem = fs::read(&opts.cert)
                .with_context(|| format!("cannot load x509 key pair from disk: {}", opts.cert))?;
            let key_pem = fs::read(&opts.key)
                .with_context(|| format!("cannot load x509 key pair from disk: {}", opts.key))?;
            Ok((cert_pem, key_pem))
        }
        _ => bail!("one of cert or key argument is missing"),
    }
}

pub async fn bind(addr: &str) -> Result<TcpListener> {
    net::bind_listener(addr)
        .await
        .with_context(|| format!("cannot bind {addr}"))
}
