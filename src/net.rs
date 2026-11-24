use socket2::{Domain, Protocol, SockRef, Socket, Type};
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};

const LOOPBACK_BUF_SIZE: i32 = 64 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const LISTEN_BACKLOG: i32 = 1024;

pub async fn connect(addr: &str, socket_buf: i32) -> io::Result<TcpStream> {
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "connect timeout"))??;
    apply_socket_buf(&stream, socket_buf);
    Ok(stream)
}

pub fn apply_socket_buf(stream: &TcpStream, user_config: i32) {
    let loopback = stream
        .local_addr()
        .map(|addr| addr.ip().is_loopback())
        .unwrap_or(false)
        || stream
            .peer_addr()
            .map(|addr| addr.ip().is_loopback())
            .unwrap_or(false);

    let size = if loopback {
        LOOPBACK_BUF_SIZE
    } else if user_config > 0 {
        user_config
    } else {
        return;
    };

    let socket = SockRef::from(stream);
    let _ = socket.set_recv_buffer_size(size as usize);
    let _ = socket.set_send_buffer_size(size as usize);
}

pub fn default_server_name(dst: &str) -> String {
    if let Ok(addr) = dst.parse::<SocketAddr>() {
        return addr.ip().to_string();
    }

    match dst.rsplit_once(':') {
        Some((host, _)) => host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned(),
        None => dst.to_owned(),
    }
}

/// Binds a listening TCP socket, matching the behaviour of Go's
/// `net.Listen("tcp", addr)`.
///
/// An empty host (`:443`) binds to the IPv6 wildcard with `IPV6_V6ONLY`
/// disabled, so a single socket accepts both IPv4 and IPv6 connections. Go does
/// the same; binding `0.0.0.0:443` instead would only accept IPv4.
pub async fn bind_listener(addr: &str) -> io::Result<TcpListener> {
    let addr = addr.trim();

    // Go treats an empty host as the dual-stack IPv6 wildcard.
    if let Some(port) = addr.strip_prefix(':') {
        if let Ok(port) = port.parse::<u16>() {
            return bind_wildcard(port);
        }
    }

    let target = addr
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no address to bind"))?;
    let dual_stack = target.is_ipv6() && target.ip().is_unspecified();
    bind_socket(target, dual_stack)
}

/// Binds the wildcard address, preferring a dual-stack IPv6 socket and falling
/// back to IPv4 when the system has no IPv6 support.
fn bind_wildcard(port: u16) -> io::Result<TcpListener> {
    let v6 = SocketAddr::from((Ipv6Addr::UNSPECIFIED, port));
    match bind_socket(v6, true) {
        Ok(listener) => Ok(listener),
        Err(error) => {
            log::debug!("cannot bind {v6}: {error}; falling back to IPv4");
            let v4 = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
            bind_socket(v4, false)
        }
    }
}

fn bind_socket(addr: SocketAddr, dual_stack: bool) -> io::Result<TcpListener> {
    let domain = if addr.is_ipv6() {
        Domain::IPV6
    } else {
        Domain::IPV4
    };
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
    if addr.is_ipv6() {
        // Must be set before bind. On Windows `IPV6_V6ONLY` defaults to 1,
        // which would hide IPv4 clients from a `[::]` socket.
        socket.set_only_v6(!dual_stack)?;
    }
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(LISTEN_BACKLOG)?;
    TcpListener::from_std(socket.into())
}
