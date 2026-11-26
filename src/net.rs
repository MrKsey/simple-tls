use socket2::{Domain, Protocol, SockRef, Socket, TcpKeepalive, Type};
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};

const LOOPBACK_BUF_SIZE: i32 = 64 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const LISTEN_BACKLOG: i32 = 1024;
const KEEPALIVE_TIME: Duration = Duration::from_secs(60);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
const ACCEPT_BACKOFF_MIN: Duration = Duration::from_millis(10);
const ACCEPT_BACKOFF_MAX: Duration = Duration::from_secs(1);

pub async fn connect(addr: &str, socket_buf: i32) -> io::Result<TcpStream> {
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "connect timeout"))??;
    apply_socket_buf(&stream, socket_buf);
    Ok(stream)
}

/// Accepts the next connection, retrying while the process or the system is
/// out of resources.
///
/// `accept` fails with `EMFILE`/`ENFILE`/`ENOBUFS` when the descriptor table or
/// the socket buffers are exhausted. Those conditions are temporary: failing
/// the whole listener on them would kill a long-running proxy, so back off and
/// retry instead, like Go's `net.Listener` users do.
pub async fn accept(listener: &TcpListener) -> io::Result<(TcpStream, SocketAddr)> {
    let mut delay = ACCEPT_BACKOFF_MIN;
    loop {
        match listener.accept().await {
            Ok(pair) => return Ok(pair),
            Err(error) if is_temporary_accept_error(&error) => {
                log::warn!(
                    "accept failed: {error}; retrying in {} ms",
                    delay.as_millis()
                );
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(ACCEPT_BACKOFF_MAX);
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(unix)]
fn is_temporary_accept_error(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(
            libc::EINTR
                | libc::ENOMEM
                | libc::ENFILE
                | libc::EMFILE
                | libc::ECONNABORTED
                | libc::ENOBUFS
        )
    )
}

#[cfg(windows)]
fn is_temporary_accept_error(error: &io::Error) -> bool {
    const WSAEMFILE: i32 = 10024;
    const WSAENOBUFS: i32 = 10055;
    matches!(error.raw_os_error(), Some(WSAEMFILE | WSAENOBUFS))
}

pub fn apply_socket_buf(stream: &TcpStream, user_config: i32) {
    let socket = SockRef::from(stream);
    // Go enables TCP keepalive by default. Without it a peer that disappears
    // without a FIN (rebooted router, mobile network change) keeps its socket
    // and file descriptor for hours.
    let _ = socket.set_tcp_keepalive(
        &TcpKeepalive::new()
            .with_time(KEEPALIVE_TIME)
            .with_interval(KEEPALIVE_INTERVAL),
    );

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
    // Go sets SO_REUSEADDR on Unix. Without it a restart right after the
    // previous process was killed can fail with "address in use" while
    // orphaned sockets are still in FIN-WAIT.
    #[cfg(unix)]
    socket.set_reuse_address(true)?;
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
