use super::outbound::{overload_frame, OutboundControl, OutboundReceiver};
use super::ImageArtifactOutbound;
use std::io;
use std::time::Duration;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch};

const BATCH_BYTES: usize = 64 * 1024;
const BATCH_FRAMES: usize = 64;
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) async fn run_outbound_writer<W>(
    writer: W,
    outbound: OutboundReceiver,
    image_artifact_outbound: mpsc::Receiver<ImageArtifactOutbound>,
    finish: oneshot::Receiver<()>,
) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    run_outbound_writer_with_timeout(
        writer,
        outbound,
        image_artifact_outbound,
        finish,
        DRAIN_TIMEOUT,
    )
    .await
}

pub(crate) async fn run_outbound_writer_with_timeout<W>(
    mut writer: W,
    mut outbound: OutboundReceiver,
    mut images: mpsc::Receiver<ImageArtifactOutbound>,
    mut finish: oneshot::Receiver<()>,
    drain_timeout: Duration,
) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let control = outbound.control();
    let (draining, receiver) = watch::channel(false);
    let pump = pump(&mut writer, &mut outbound, &mut images, receiver, &control);
    tokio::pin!(pump);
    let mut deadline = None;
    let result = loop {
        tokio::select! {
            result = &mut pump => break result,
            _ = &mut finish, if deadline.is_none() => {
                control.close();
                let _ = draining.send(true);
                deadline = Some(tokio::time::Instant::now() + drain_timeout);
            }
            _ = control.failed(), if deadline.is_none() => {
                control.close();
                let _ = draining.send(true);
                deadline = Some(tokio::time::Instant::now() + drain_timeout);
            }
            _ = async {
                match deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            } => break Err(io::Error::new(io::ErrorKind::TimedOut,
                "outbound transport could not drain before shutdown deadline")),
        }
    };
    if result.is_err() {
        control.writer_failed();
    }
    result
}

async fn pump<W>(
    writer: &mut W,
    outbound: &mut OutboundReceiver,
    images: &mut mpsc::Receiver<ImageArtifactOutbound>,
    mut draining: watch::Receiver<bool>,
    control: &OutboundControl,
) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut normal_open = true;
    let mut images_open = true;
    let mut closing = false;
    loop {
        if *draining.borrow() && !closing {
            closing = true;
            images.close();
            outbound.close();
        }
        if !normal_open && !images_open {
            if control.overloaded() {
                writer.write_all(overload_frame()).await?;
                writer.flush().await?;
                writer.shutdown().await?;
                return Err(io::Error::new(
                    io::ErrorKind::OutOfMemory,
                    "outbound_overloaded: bounded output queue exhausted",
                ));
            }
            writer.shutdown().await?;
            return Ok(());
        }
        tokio::select! {
            _ = draining.changed(), if !closing => {},
            message = images.recv(), if images_open => {
                match message {
                    Some(message) => {
                        // The artifact permit stays live through the flush, as before. A single
                        // large JSON-RPC line cannot be preempted without changing the protocol.
                        let mut bytes = serde_json::to_vec(&message.message)?;
                        bytes.push(b'\n');
                        writer.write_all(&bytes).await?;
                        writer.flush().await?;
                        drop(message);
                    }
                    None => images_open = false,
                }
            }
            frame = outbound.recv_frame(), if normal_open => {
                match frame {
                    Some(frame) => {
                        let mut size = frame.wire.len();
                        let mut frames = vec![frame];
                        // No timer window: the first character of a low-rate stream flushes now.
                        // Only the backlog already ready at this instant joins this write batch.
                        while frames.len() < BATCH_FRAMES && size < BATCH_BYTES {
                            let Ok(next) = outbound.try_recv_frame(BATCH_BYTES - size) else { break; };
                            size += next.wire.len();
                            frames.push(next);
                        }
                        if frames.len() == 1 {
                            writer.write_all(&frames[0].wire).await?;
                        } else {
                            let mut batch = Vec::with_capacity(size);
                            for frame in &frames { batch.extend_from_slice(&frame.wire); }
                            writer.write_all(&batch).await?;
                        }
                        writer.flush().await?;
                        // In particular, do not release an oversized response lease before flush.
                        drop(frames);
                    }
                    None => normal_open = false,
                }
            }
        }
    }
}
