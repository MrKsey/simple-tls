use anyhow::{anyhow, bail, Context, Result};
use bytes::{Buf, Bytes, BytesMut};
use h2::client::SendRequest;
use h2::{RecvStream, SendStream};
use http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode, Uri};
use rustls::pki_types::ServerName;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::task::{ready, Context as TaskContext, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_rustls::TlsConnector;

use crate::net;
use crate::tunnel;

const MAX_MESSAGE_SIZE: usize = 128 * 1024;
const MAX_STREAMS_PER_CONN: usize = 4;
const CONN_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const GRPC_FRAME_OVERHEAD: usize = 16;
const GRPC_WRITE_CHUNK: usize = 32 * 1024;
const INITIAL_WINDOW_SIZE: u32 = 1024 * 1024;
const MAX_CONCURRENT_STREAMS: u32 = 64;

#[derive(Debug, Clone)]
pub struct Route {
    pub path: String,
    pub dst: String,
}

pub fn build_routes(dst: &str, service_name: &str) -> Result<Vec<Route>> {
    if dst.contains('/') || dst.contains(',') {
        let mut routes = Vec::new();
        for peer in dst.split(',') {
            let Some((path, dst)) = peer.split_once('/') else {
                bail!("invalid dst value [{peer}]");
            };
            let path = format!("/{path}/Connect");
            log::info!("starting grpc func at path {path} -> {dst}");
            routes.push(Route {
                path,
                dst: dst.to_owned(),
            });
        }
        Ok(routes)
    } else {
        Ok(vec![Route {
            path: format!("/{service_name}/Connect"),
            dst: dst.to_owned(),
        }])
    }
}

pub async fn serve_connection(
    tls: tokio_rustls::server::TlsStream<TcpStream>,
    routes: Arc<Vec<Route>>,
    idle_timeout: Duration,
    outbound_buf: i32,
) -> Result<()> {
    let mut builder = h2::server::Builder::new();
    builder
        .initial_window_size(INITIAL_WINDOW_SIZE)
        .initial_connection_window_size(INITIAL_WINDOW_SIZE)
        .max_concurrent_streams(MAX_CONCURRENT_STREAMS);

    let mut connection = builder.handshake(tls).await?;
    while let Some(result) = connection.accept().await {
        let (request, respond) = result?;
        let routes = routes.clone();
        tokio::spawn(async move {
            if let Err(error) =
                handle_request(request, respond, routes, idle_timeout, outbound_buf).await
            {
                log::debug!("grpc request failed: {error:#}");
            }
        });
    }

    Ok(())
}

async fn handle_request(
    request: Request<RecvStream>,
    mut respond: h2::server::SendResponse<Bytes>,
    routes: Arc<Vec<Route>>,
    idle_timeout: Duration,
    outbound_buf: i32,
) -> Result<()> {
    let path = request.uri().path().to_owned();
    let route = routes.iter().find(|route| route.path == path).cloned();

    let Some(route) = route else {
        let mut response = Response::new(());
        *response.status_mut() = StatusCode::OK;
        response
            .headers_mut()
            .insert(http::header::CONTENT_TYPE, grpc_content_type());
        let mut send = respond.send_response(response, false)?;
        let _ = send_trailers(&mut send, 12, Some("unimplemented"));
        return Ok(());
    };

    if request.method() != Method::POST {
        let mut response = Response::new(());
        *response.status_mut() = StatusCode::OK;
        response
            .headers_mut()
            .insert(http::header::CONTENT_TYPE, grpc_content_type());
        let mut send = respond.send_response(response, false)?;
        let _ = send_trailers(&mut send, 12, Some("method not allowed"));
        return Ok(());
    }

    let mut recv = request.into_body();
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(http::header::CONTENT_TYPE, "application/grpc")
        .body(())?;
    let mut send = respond.send_response(response, false)?;

    let dst = match net::connect(&route.dst, outbound_buf).await {
        Ok(dst) => dst,
        Err(error) => {
            let _ = send_trailers(
                &mut send,
                14,
                Some(&format!("cannot connect to the dst: {error}")),
            );
            return Ok(());
        }
    };

    let (mut dst_reader, mut dst_writer) = dst.into_split();
    let mut read_buf = BytesMut::new();
    let mut dst_buf = vec![0u8; tunnel::GRPC_BUF_SIZE];
    let mut trailers_sent = false;
    let last_activity = Arc::new(AtomicU64::new(tunnel::now_millis()));

    let grpc_to_dst_activity = last_activity.clone();
    let grpc_to_dst = async {
        while let Some(message) = read_grpc_message(&mut recv, &mut read_buf).await? {
            if !message.is_empty() {
                grpc_to_dst_activity.store(tunnel::now_millis(), Ordering::Relaxed);
                dst_writer.write_all(&message).await?;
                grpc_to_dst_activity.store(tunnel::now_millis(), Ordering::Relaxed);
            }
        }
        let _ = dst_writer.shutdown().await;
        Ok::<(), io::Error>(())
    };

    let dst_to_grpc_activity = last_activity.clone();
    let dst_to_grpc = async {
        loop {
            let n = dst_reader.read(&mut dst_buf).await?;
            if n == 0 {
                break;
            }
            dst_to_grpc_activity.store(tunnel::now_millis(), Ordering::Relaxed);
            send_grpc_data(&mut send, &dst_buf[..n]).await?;
            dst_to_grpc_activity.store(tunnel::now_millis(), Ordering::Relaxed);
        }
        send_trailers(&mut send, 0, None).map_err(grpc_io_error)?;
        Ok::<(), io::Error>(())
    };

    let idle_ms = idle_timeout.as_millis() as u64;
    if idle_ms == 0 {
        tokio::select! {
            result = grpc_to_dst => {
                if let Err(error) = result {
                    log::debug!("grpc -> dst tunnel error: {error}");
                }
            }
            result = dst_to_grpc => {
                trailers_sent = result.is_ok();
                if let Err(error) = result {
                    log::debug!("dst -> grpc tunnel error: {error}");
                }
            }
        }
    } else {
        let monitor = tunnel::idle_monitor(last_activity, idle_ms);
        tokio::select! {
            result = grpc_to_dst => {
                if let Err(error) = result {
                    log::debug!("grpc -> dst tunnel error: {error}");
                }
            }
            result = dst_to_grpc => {
                trailers_sent = result.is_ok();
                if let Err(error) = result {
                    log::debug!("dst -> grpc tunnel error: {error}");
                }
            }
            result = monitor => {
                if let Err(error) = result {
                    log::debug!("grpc stream idle timeout: {error}");
                }
            }
        }
    }

    if !trailers_sent {
        let _ = send_trailers(&mut send, 0, None);
    }

    Ok(())
}

fn grpc_content_type() -> HeaderValue {
    HeaderValue::from_static("application/grpc")
}

fn send_trailers(
    send: &mut SendStream<Bytes>,
    status: u32,
    message: Option<&str>,
) -> Result<(), h2::Error> {
    let mut trailers = HeaderMap::new();
    trailers.insert(
        "grpc-status",
        HeaderValue::from_str(&status.to_string()).expect("valid grpc status"),
    );
    if let Some(message) = message {
        let message: String = message
            .chars()
            .map(|ch| {
                if ch.is_ascii_graphic() || ch == ' ' {
                    ch
                } else {
                    ' '
                }
            })
            .collect();
        if let Ok(value) = HeaderValue::from_str(&message) {
            trailers.insert("grpc-message", value);
        }
    }
    send.send_trailers(trailers)
}

fn grpc_io_error(error: h2::Error) -> io::Error {
    io::Error::other(error)
}

/// Returns an error if the gRPC trailers carry a non-zero `grpc-status`.
fn grpc_status_error(trailers: Option<&HeaderMap>) -> Option<io::Error> {
    let trailers = trailers?;
    let status = trailers.get("grpc-status")?.to_str().ok()?;
    let status: i32 = status.parse().ok()?;
    if status == 0 {
        return None;
    }

    let message = trailers
        .get("grpc-message")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("unknown error");
    log::debug!("grpc: stream closed with status {status}: {message}");
    Some(io::Error::other(format!(
        "grpc stream closed with status {status}: {message}"
    )))
}

async fn read_grpc_message(
    recv: &mut RecvStream,
    read_buf: &mut BytesMut,
) -> io::Result<Option<Bytes>> {
    loop {
        if let Some(message) = decode_frame(read_buf)? {
            return Ok(Some(message));
        }

        match recv.data().await {
            Some(Ok(chunk)) => {
                let size = chunk.len();
                read_buf.extend_from_slice(&chunk);
                recv.flow_control()
                    .release_capacity(size)
                    .map_err(grpc_io_error)?;
            }
            Some(Err(error)) => return Err(grpc_io_error(error)),
            None => return Ok(None),
        }
    }
}

async fn send_grpc_data(send: &mut SendStream<Bytes>, mut data: &[u8]) -> io::Result<()> {
    while !data.is_empty() {
        let capacity = send.capacity();
        if capacity <= GRPC_FRAME_OVERHEAD {
            send.reserve_capacity(GRPC_WRITE_CHUNK);
            match std::future::poll_fn(|cx| send.poll_capacity(cx)).await {
                Some(Ok(_)) => continue,
                Some(Err(error)) => return Err(grpc_io_error(error)),
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "grpc stream is closed",
                    ))
                }
            }
        }

        let size = data.len().min(capacity - GRPC_FRAME_OVERHEAD);
        let frame = encode_frame(&data[..size]);
        send.send_data(frame, false).map_err(grpc_io_error)?;
        data = &data[size..];
    }
    Ok(())
}

#[derive(Clone)]
pub struct GrpcPool {
    inner: Arc<PoolInner>,
}

struct PoolInner {
    target: String,
    tls_config: Arc<rustls::ClientConfig>,
    server_name: ServerName<'static>,
    service_name: String,
    outbound_buf: i32,
    conns: Mutex<Vec<Arc<PooledConn>>>,
    dial_lock: Mutex<()>,
}

struct PooledConn {
    send: Mutex<SendRequest<Bytes>>,
    active: AtomicUsize,
    closed: AtomicBool,
}

impl GrpcPool {
    pub fn new(
        target: String,
        tls_config: Arc<rustls::ClientConfig>,
        server_name: ServerName<'static>,
        service_name: String,
        outbound_buf: i32,
    ) -> Self {
        Self {
            inner: Arc::new(PoolInner {
                target,
                tls_config,
                server_name,
                service_name,
                outbound_buf,
                conns: Mutex::new(Vec::new()),
                dial_lock: Mutex::new(()),
            }),
        }
    }

    pub async fn connect_stream(&self) -> Result<PooledGrpcStream> {
        let conn = self.acquire().await?;
        match self.open_stream(&conn).await {
            Ok(stream) => Ok(stream),
            Err(error) => {
                conn.active.fetch_sub(1, Ordering::SeqCst);
                conn.closed.store(true, Ordering::SeqCst);
                Err(error)
            }
        }
    }

    async fn acquire(&self) -> Result<Arc<PooledConn>> {
        {
            let mut conns = self.inner.conns.lock().await;
            conns.retain(|conn| !conn.closed.load(Ordering::Relaxed));
            if let Some(conn) = conns
                .iter()
                .find(|conn| conn.active.load(Ordering::Relaxed) < MAX_STREAMS_PER_CONN)
            {
                conn.active.fetch_add(1, Ordering::SeqCst);
                return Ok(conn.clone());
            }
        }

        let _dial_guard = self.inner.dial_lock.lock().await;

        {
            let mut conns = self.inner.conns.lock().await;
            conns.retain(|conn| !conn.closed.load(Ordering::Relaxed));
            if let Some(conn) = conns
                .iter()
                .find(|conn| conn.active.load(Ordering::Relaxed) < MAX_STREAMS_PER_CONN)
            {
                conn.active.fetch_add(1, Ordering::SeqCst);
                return Ok(conn.clone());
            }
        }

        let (send, connection) = self.dial().await?;
        let conn = Arc::new(PooledConn {
            send: Mutex::new(send),
            active: AtomicUsize::new(1),
            closed: AtomicBool::new(false),
        });

        self.inner.conns.lock().await.push(conn.clone());

        let connection_conn = conn.clone();
        tokio::spawn(async move {
            if let Err(error) = connection.await {
                log::debug!("grpc client connection closed: {error}");
            }
            connection_conn.closed.store(true, Ordering::SeqCst);
        });

        tokio::spawn(idle_monitor(
            Arc::downgrade(&self.inner),
            conn.clone(),
            CONN_IDLE_TIMEOUT,
        ));

        Ok(conn)
    }

    async fn dial(
        &self,
    ) -> Result<(
        SendRequest<Bytes>,
        h2::client::Connection<tokio_rustls::client::TlsStream<TcpStream>, Bytes>,
    )> {
        let tcp = net::connect(&self.inner.target, self.inner.outbound_buf)
            .await
            .with_context(|| format!("cannot connect to {}", self.inner.target))?;
        let connector = TlsConnector::from(self.inner.tls_config.clone());
        let tls = tokio::time::timeout(
            Duration::from_secs(5),
            connector.connect(self.inner.server_name.clone(), tcp),
        )
        .await
        .map_err(|_| anyhow!("TLS handshake timeout"))?
        .context("TLS handshake failed")?;

        let mut builder = h2::client::Builder::new();
        builder
            .initial_window_size(INITIAL_WINDOW_SIZE)
            .initial_connection_window_size(INITIAL_WINDOW_SIZE);

        let (send, connection) = builder
            .handshake(tls)
            .await
            .context("HTTP/2 handshake failed")?;
        Ok((send, connection))
    }

    async fn open_stream(&self, conn: &Arc<PooledConn>) -> Result<PooledGrpcStream> {
        let path = format!("/{}/Connect", self.inner.service_name);
        let uri = Uri::builder()
            .scheme("https")
            .authority(self.inner.target.as_str())
            .path_and_query(path.as_str())
            .build()
            .context("failed to build grpc URI")?;

        let request = Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header(http::header::CONTENT_TYPE, "application/grpc")
            .header("te", "trailers")
            .header(
                http::header::USER_AGENT,
                concat!("simple-tls/", env!("CARGO_PKG_VERSION")),
            )
            .body(())
            .context("failed to build grpc request")?;

        let (response_future, send) = {
            let mut sender = conn.send.lock().await;
            sender
                .send_request(request, false)
                .context("failed to send grpc request")?
        };

        // Do not wait for the response headers here. grpc-go servers send them
        // lazily, only once the first message arrives (or the handler returns),
        // so blocking on the response before the tunnel can send data would
        // deadlock against such servers.
        Ok(PooledGrpcStream {
            inner: GrpcStream::new(response_future, send),
            conn: conn.clone(),
        })
    }
}

async fn idle_monitor(inner: Weak<PoolInner>, conn: Arc<PooledConn>, idle_timeout: Duration) {
    loop {
        tokio::time::sleep(idle_timeout).await;
        if conn.active.load(Ordering::Relaxed) != 0 {
            continue;
        }

        let Some(inner) = inner.upgrade() else {
            break;
        };
        let mut conns = inner.conns.lock().await;
        if conn.active.load(Ordering::Relaxed) == 0 {
            conns.retain(|item| !Arc::ptr_eq(item, &conn));
            conn.closed.store(true, Ordering::SeqCst);
            break;
        }
    }
}

pub struct PooledGrpcStream {
    inner: GrpcStream,
    conn: Arc<PooledConn>,
}

impl Drop for PooledGrpcStream {
    fn drop(&mut self) {
        self.conn.active.fetch_sub(1, Ordering::SeqCst);
    }
}

impl AsyncRead for PooledGrpcStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for PooledGrpcStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

struct GrpcStream {
    response: Option<h2::client::ResponseFuture>,
    recv: Option<RecvStream>,
    send: SendStream<Bytes>,
    read_buf: BytesMut,
    current: Bytes,
    eof: bool,
}

impl GrpcStream {
    fn new(response: h2::client::ResponseFuture, send: SendStream<Bytes>) -> Self {
        Self {
            response: Some(response),
            recv: None,
            send,
            read_buf: BytesMut::new(),
            current: Bytes::new(),
            eof: false,
        }
    }

    fn poll_response(&mut self, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        let Some(response) = &mut self.response else {
            return Poll::Ready(Ok(()));
        };

        match Pin::new(response).poll(cx) {
            Poll::Ready(Ok(response)) => {
                self.response = None;
                if response.status() != StatusCode::OK {
                    return Poll::Ready(Err(io::Error::other(format!(
                        "grpc server returned status {}",
                        response.status()
                    ))));
                }
                self.recv = Some(response.into_body());
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(error)) => {
                self.response = None;
                Poll::Ready(Err(io::Error::other(format!(
                    "grpc request failed: {error}"
                ))))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Unpin for GrpcStream {}

impl AsyncRead for GrpcStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();

        loop {
            if buf.remaining() == 0 {
                return Poll::Ready(Ok(()));
            }

            if !this.current.is_empty() {
                let size = buf.remaining().min(this.current.len());
                buf.put_slice(&this.current[..size]);
                this.current.advance(size);
                return Poll::Ready(Ok(()));
            }

            match decode_frame(&mut this.read_buf) {
                Ok(Some(message)) => {
                    this.current = message;
                    continue;
                }
                Ok(None) => {}
                Err(error) => return Poll::Ready(Err(error)),
            }

            if this.eof {
                return Poll::Ready(Ok(()));
            }

            ready!(this.poll_response(cx))?;
            let Some(recv) = this.recv.as_mut() else {
                return Poll::Ready(Err(io::Error::other("grpc stream is not ready")));
            };

            match recv.poll_data(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    let size = chunk.len();
                    this.read_buf.extend_from_slice(&chunk);
                    if let Err(error) = recv.flow_control().release_capacity(size) {
                        return Poll::Ready(Err(grpc_io_error(error)));
                    }
                }
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Err(grpc_io_error(error))),
                Poll::Ready(None) => match recv.poll_trailers(cx) {
                    Poll::Ready(Ok(trailers)) => {
                        if let Some(status) = grpc_status_error(trailers.as_ref()) {
                            return Poll::Ready(Err(status));
                        }
                        this.eof = true;
                    }
                    Poll::Ready(Err(error)) => return Poll::Ready(Err(grpc_io_error(error))),
                    Poll::Pending => return Poll::Pending,
                },
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl AsyncWrite for GrpcStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }

        let mut capacity = this.send.capacity();
        if capacity <= GRPC_FRAME_OVERHEAD {
            this.send.reserve_capacity(GRPC_WRITE_CHUNK);
            match this.send.poll_capacity(cx) {
                Poll::Ready(Some(Ok(_))) => {
                    capacity = this.send.capacity();
                }
                Poll::Ready(Some(Err(error))) => {
                    return Poll::Ready(Err(grpc_io_error(error)));
                }
                Poll::Ready(None) => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "grpc stream is closed",
                    )));
                }
                Poll::Pending => return Poll::Pending,
            }
        }

        if capacity <= GRPC_FRAME_OVERHEAD {
            this.send.reserve_capacity(GRPC_WRITE_CHUNK);
            return match this.send.poll_capacity(cx) {
                Poll::Ready(Some(Ok(_))) => {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
                Poll::Ready(Some(Err(error))) => Poll::Ready(Err(grpc_io_error(error))),
                Poll::Ready(None) => Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "grpc stream is closed",
                ))),
                Poll::Pending => Poll::Pending,
            };
        }

        let size = data.len().min(capacity - GRPC_FRAME_OVERHEAD);
        let frame = encode_frame(&data[..size]);
        match this.send.send_data(frame, false) {
            Ok(()) => Poll::Ready(Ok(size)),
            Err(error) => Poll::Ready(Err(grpc_io_error(error))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let _ = this.send.send_data(Bytes::new(), true);
        Poll::Ready(Ok(()))
    }
}

fn encode_frame(payload: &[u8]) -> Bytes {
    let mut message = Vec::with_capacity(payload.len() + 10);
    message.push(0x0A);
    put_varint(payload.len(), &mut message);
    message.extend_from_slice(payload);

    let mut frame = Vec::with_capacity(message.len() + 5);
    frame.push(0);
    frame.extend_from_slice(&(message.len() as u32).to_be_bytes());
    frame.extend_from_slice(&message);
    Bytes::from(frame)
}

fn decode_frame(read_buf: &mut BytesMut) -> io::Result<Option<Bytes>> {
    if read_buf.len() < 5 {
        return Ok(None);
    }

    if read_buf[0] != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "compressed gRPC messages are not supported",
        ));
    }

    let length = u32::from_be_bytes([read_buf[1], read_buf[2], read_buf[3], read_buf[4]]) as usize;
    if length > MAX_MESSAGE_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "gRPC message is too large",
        ));
    }
    if read_buf.len() < 5 + length {
        return Ok(None);
    }

    read_buf.advance(5);
    let message = read_buf.split_to(length).freeze();
    decode_proto(&message).map(Some)
}

fn decode_proto(message: &Bytes) -> io::Result<Bytes> {
    let mut offset = 0usize;
    let mut payload = Bytes::new();

    while offset < message.len() {
        let (key, key_len) = read_varint(&message[offset..])
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid protobuf varint"))?;
        offset += key_len;

        let field = key >> 3;
        let wire_type = key & 0x07;

        if field == 1 && wire_type == 2 {
            let (length, length_len) = read_varint(&message[offset..]).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid protobuf length")
            })?;
            offset += length_len;
            if offset + length > message.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "truncated protobuf message",
                ));
            }
            payload = message.slice(offset..offset + length);
            offset += length;
            continue;
        }

        match wire_type {
            0 => {
                let (_, length) = read_varint(&message[offset..]).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid protobuf varint")
                })?;
                offset += length;
            }
            1 => offset += 8,
            2 => {
                let (length, length_len) = read_varint(&message[offset..]).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid protobuf length")
                })?;
                offset += length_len + length;
            }
            5 => offset += 4,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unsupported protobuf wire type",
                ))
            }
        }

        if offset > message.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "truncated protobuf message",
            ));
        }
    }

    Ok(payload)
}

fn put_varint(mut value: usize, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push(((value & 0x7f) as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn read_varint(buf: &[u8]) -> Option<(usize, usize)> {
    let mut value = 0usize;
    let mut shift = 0usize;
    for (index, byte) in buf.iter().copied().enumerate() {
        if shift >= usize::BITS as usize {
            return None;
        }
        value |= ((byte & 0x7f) as usize) << shift;
        if byte & 0x80 == 0 {
            return Some((value, index + 1));
        }
        shift += 7;
    }
    None
}
