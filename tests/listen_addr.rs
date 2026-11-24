//! The listen address handling must match Go's `net.Listen("tcp", ...)`:
//! an empty host means "all interfaces", both IPv4 and IPv6.

use std::time::Duration;
use tokio::net::{TcpStream, ToSocketAddrs};

async fn connect(addr: impl ToSocketAddrs) -> std::io::Result<TcpStream> {
    tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(addr))
        .await
        .expect("connect timed out")
}

#[tokio::test]
async fn wildcard_bind_accepts_ipv4() {
    let listener = simple_tls::net::bind_listener(":0").await.expect("bind :0");
    let port = listener.local_addr().expect("local addr").port();
    assert_ne!(port, 0);

    let (client, accepted) = tokio::join!(connect(("127.0.0.1", port)), listener.accept());
    client.expect("connect over IPv4");
    accepted.expect("accept over IPv4");
}

#[tokio::test]
async fn wildcard_bind_accepts_ipv6_when_available() {
    let listener = simple_tls::net::bind_listener(":0").await.expect("bind :0");
    let local = listener.local_addr().expect("local addr");
    if !local.is_ipv6() {
        // The system has no IPv6 support and the bind fell back to IPv4.
        return;
    }
    let port = local.port();

    let (client, accepted) = tokio::join!(connect(("::1", port)), listener.accept());
    client.expect("connect over IPv6");
    accepted.expect("accept over IPv6");
}

#[tokio::test]
async fn explicit_ipv4_wildcard_stays_ipv4_only() {
    let listener = simple_tls::net::bind_listener("0.0.0.0:0")
        .await
        .expect("bind 0.0.0.0:0");
    let local = listener.local_addr().expect("local addr");
    assert!(local.is_ipv4());

    let (client, accepted) = tokio::join!(connect(("127.0.0.1", local.port())), listener.accept());
    client.expect("connect over IPv4");
    accepted.expect("accept over IPv4");
}
