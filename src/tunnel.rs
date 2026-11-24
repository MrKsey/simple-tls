use portable_atomic::AtomicU64;
use std::io;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const RAW_BUF_SIZE: usize = 64 * 1024;
pub const GRPC_BUF_SIZE: usize = 32 * 1024;

/// Opens a bidirectional tunnel between `a` and `b`.
///
/// Both sides are closed as soon as either direction finishes, matching the
/// behaviour of the original Go implementation. `idle_timeout` closes the
/// connection when no data has moved in either direction.
pub async fn tunnel<A, B>(a: A, b: B, idle_timeout: Duration, buf_size: usize) -> io::Result<()>
where
    A: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    B: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut ar, mut aw) = tokio::io::split(a);
    let (mut br, mut bw) = tokio::io::split(b);

    let last_activity = Arc::new(AtomicU64::new(now_millis()));
    let a_to_b_bytes = Arc::new(AtomicU64::new(0));
    let b_to_a_bytes = Arc::new(AtomicU64::new(0));

    let a_to_b = async {
        let result = copy_direction(
            &mut ar,
            &mut bw,
            last_activity.clone(),
            a_to_b_bytes.clone(),
            buf_size,
        )
        .await;
        log::debug!("tunnel a->b finished: {result:?}");
        result.map(|_| ())
    };
    let b_to_a = async {
        let result = copy_direction(
            &mut br,
            &mut aw,
            last_activity.clone(),
            b_to_a_bytes.clone(),
            buf_size,
        )
        .await;
        log::debug!("tunnel b->a finished: {result:?}");
        result.map(|_| ())
    };

    let idle = idle_timeout.as_millis() as u64;
    let result = if idle == 0 {
        tokio::select! {
            result = a_to_b => result,
            result = b_to_a => result,
        }
    } else {
        let monitor = idle_monitor(last_activity.clone(), idle);
        tokio::select! {
            result = a_to_b => result,
            result = b_to_a => result,
            result = monitor => {
                log::debug!(
                    "tunnel idle timeout after a->b={} b->a={} bytes",
                    a_to_b_bytes.load(Ordering::Relaxed),
                    b_to_a_bytes.load(Ordering::Relaxed)
                );
                result
            },
        }
    };

    // Best-effort graceful shutdown: send TLS close_notify and TCP FIN on both
    // sides instead of dropping the sockets abruptly. A misbehaving peer must
    // not keep the tunnel task alive forever.
    let _ = tokio::time::timeout(Duration::from_secs(1), async {
        let _ = tokio::join!(aw.shutdown(), bw.shutdown());
    })
    .await;

    result
}

async fn copy_direction<R, W>(
    reader: &mut R,
    writer: &mut W,
    last_activity: Arc<AtomicU64>,
    copied: Arc<AtomicU64>,
    buf_size: usize,
) -> io::Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; buf_size.max(1024)];
    let mut total = 0u64;
    loop {
        let n = match reader.read(&mut buf).await {
            Ok(n) => n,
            Err(error) => {
                log::debug!("tunnel direction error after {total} bytes: {error}");
                return Err(error);
            }
        };
        if n == 0 {
            log::debug!("tunnel direction EOF after {total} bytes");
            return Ok(total);
        }
        last_activity.store(now_millis(), Ordering::Relaxed);
        writer.write_all(&buf[..n]).await?;
        // TLS writers may buffer ciphertext internally even after `write_all`
        // accepts the plaintext, so flush explicitly before waiting for more
        // input. Otherwise the tail of a transfer can stay buffered forever.
        writer.flush().await?;
        total += n as u64;
        copied.store(total, Ordering::Relaxed);
        last_activity.store(now_millis(), Ordering::Relaxed);
    }
}

pub(crate) async fn idle_monitor(last_activity: Arc<AtomicU64>, idle_ms: u64) -> io::Result<()> {
    let check_interval = Duration::from_millis((idle_ms / 2).max(100));
    loop {
        tokio::time::sleep(check_interval).await;
        let elapsed = now_millis().saturating_sub(last_activity.load(Ordering::Relaxed));
        if elapsed >= idle_ms {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "idle timeout"));
        }
    }
}

pub(crate) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
