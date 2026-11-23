use socket2::SockRef;
use std::io;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpStream;

const LOOPBACK_BUF_SIZE: i32 = 64 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

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

pub fn normalize_bind_addr(addr: &str) -> String {
    if let Some(port) = addr.strip_prefix(':') {
        format!("0.0.0.0:{port}")
    } else {
        addr.to_owned()
    }
}
