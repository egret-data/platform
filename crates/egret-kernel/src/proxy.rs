//! `KernelProxy` trait + `BackendWsProxy` — proxies the Jupyter channels WebSocket
//! between the client and the gateway. Behind a trait so a route-table proxy can swap in.

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::{Sink, SinkExt, Stream, StreamExt};

use crate::error::KernelError;

type AxMessage = axum::extract::ws::Message;
type TgMessage = tokio_tungstenite::tungstenite::Message;

#[async_trait]
pub trait KernelProxy: Send + Sync {
    /// Proxy the upgraded client WebSocket to the gateway's kernel channels,
    /// pumping both ways until either side closes. `tap.on_frame` fires per
    /// forwarded frame (both directions) with the frame bytes, so the caller
    /// can touch the kernel and record kernel outputs.
    async fn proxy_channels(
        &self,
        client: axum::extract::ws::WebSocket,
        gateway_url: &str,
        kernel_id: &str,
        tap: Arc<dyn crate::recorder::ChannelTap>,
    ) -> Result<(), KernelError>;
}

/// Proxy-in-backend implementation (sparklabx-style).
pub struct BackendWsProxy;

#[async_trait]
impl KernelProxy for BackendWsProxy {
    async fn proxy_channels(
        &self,
        client: axum::extract::ws::WebSocket,
        gateway_url: &str,
        kernel_id: &str,
        tap: Arc<dyn crate::recorder::ChannelTap>,
    ) -> Result<(), KernelError> {
        let (tx, rx) = client.split();
        run_proxy(tx, rx, gateway_url, kernel_id, tap).await
    }
}

/// The pump core, generic over the client sink/stream so it is testable with
/// in-memory channels (the real call passes the split axum WebSocket halves).
pub(crate) async fn run_proxy<CTx, CRx>(
    mut client_tx: CTx,
    mut client_rx: CRx,
    gateway_url: &str,
    kernel_id: &str,
    tap: Arc<dyn crate::recorder::ChannelTap>,
) -> Result<(), KernelError>
where
    CTx: Sink<AxMessage> + Unpin,
    CRx: Stream<Item = Result<AxMessage, axum::Error>> + Unpin,
{
    // http(s)://host → ws(s)://host, then the Jupyter channels path.
    let ws_url = format!(
        "{}/api/kernels/{}/channels",
        gateway_url
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1),
        kernel_id
    );
    let (gw, _resp) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .map_err(|e| KernelError::Gateway(format!("connect channels: {e}")))?;
    let (mut gw_tx, mut gw_rx) = gw.split();

    loop {
        tokio::select! {
            c = client_rx.next() => match c {
                Some(Ok(msg)) => {
                    if let Some(b) = ax_frame_bytes(&msg) {
                        tap.on_frame(crate::recorder::FrameDir::ClientToKernel, b);
                    }
                    if let Some(out) = ax_to_tung(msg) {
                        if gw_tx.send(out).await.is_err() { break; }
                    }
                }
                _ => {
                    // client gone → tell the gateway, then stop
                    let _ = gw_tx.send(TgMessage::Close(None)).await;
                    break;
                }
            },
            g = gw_rx.next() => match g {
                Some(Ok(msg)) => {
                    if let Some(b) = tg_frame_bytes(&msg) {
                        tap.on_frame(crate::recorder::FrameDir::KernelToClient, b);
                    }
                    if let Some(out) = tung_to_ax(msg) {
                        if client_tx.send(out).await.is_err() { break; }
                    }
                }
                _ => {
                    // gateway gone → tell the client, then stop
                    let _ = client_tx.send(AxMessage::Close(None)).await;
                    break;
                }
            },
        }
    }
    Ok(())
}

/// Map an axum client message to a tungstenite (gateway) message.
/// Returns None for variants that should not be forwarded as-is.
fn ax_to_tung(m: AxMessage) -> Option<TgMessage> {
    use axum::extract::ws::Message as A;
    Some(match m {
        A::Text(t) => TgMessage::Text(t.as_str().to_owned().into()),
        A::Binary(b) => TgMessage::Binary(b.to_vec().into()),
        A::Ping(b) => TgMessage::Ping(b.to_vec().into()),
        A::Pong(b) => TgMessage::Pong(b.to_vec().into()),
        A::Close(Some(cf)) => {
            use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
            use tokio_tungstenite::tungstenite::protocol::CloseFrame as TgClose;
            TgMessage::Close(Some(TgClose {
                code: CloseCode::from(cf.code),
                reason: cf.reason.as_str().to_owned().into(),
            }))
        }
        A::Close(None) => TgMessage::Close(None),
    })
}

/// Map a tungstenite (gateway) message to an axum client message.
fn tung_to_ax(m: TgMessage) -> Option<AxMessage> {
    use axum::extract::ws::Message as A;
    use tokio_tungstenite::tungstenite::Message as T;
    Some(match m {
        T::Text(t) => A::Text(t.as_str().to_owned().into()),
        T::Binary(b) => A::Binary(b.to_vec().into()),
        T::Ping(b) => A::Ping(b.to_vec().into()),
        T::Pong(b) => A::Pong(b.to_vec().into()),
        T::Close(Some(cf)) => {
            use axum::extract::ws::CloseFrame as AxClose;
            A::Close(Some(AxClose {
                code: u16::from(cf.code),
                reason: cf.reason.as_str().to_owned().into(),
            }))
        }
        T::Close(None) => A::Close(None),
        T::Frame(_) => return None,
    })
}

/// Borrow the raw payload bytes of an axum (client) message for the tap, without
/// consuming the message (forwarding still gets the original verbatim). Only
/// Text/Binary carry recordable Jupyter frames; control frames return None.
fn ax_frame_bytes(m: &AxMessage) -> Option<&[u8]> {
    use axum::extract::ws::Message as A;
    match m {
        A::Text(t) => Some(t.as_bytes()),
        A::Binary(b) => Some(&b[..]),
        _ => None,
    }
}

/// Borrow the raw payload bytes of a tungstenite (gateway) message for the tap,
/// without consuming it. Only Text/Binary carry recordable Jupyter frames.
fn tg_frame_bytes(m: &TgMessage) -> Option<&[u8]> {
    use tokio_tungstenite::tungstenite::Message as T;
    match m {
        T::Text(t) => Some(t.as_bytes()),
        T::Binary(b) => Some(&b[..]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use std::sync::atomic::{AtomicUsize, Ordering};

    type AxMessage = axum::extract::ws::Message;

    /// A `ChannelTap` that counts every forwarded frame (both directions).
    struct CountTap {
        count: std::sync::Arc<AtomicUsize>,
    }
    impl crate::recorder::ChannelTap for CountTap {
        fn on_frame(&self, _dir: crate::recorder::FrameDir, _bytes: &[u8]) {
            self.count.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// A WS echo server standing in for the gateway's channels endpoint.
    /// Accepts any path, echoes text/binary frames back, stops on Close.
    async fn spawn_echo_ws() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                while let Some(Ok(msg)) = ws.next().await {
                    if msg.is_close() {
                        break;
                    }
                    if (msg.is_text() || msg.is_binary()) && ws.send(msg).await.is_err() {
                        break;
                    }
                }
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn proxy_forwards_both_ways_and_counts_activity() {
        let gateway = spawn_echo_ws().await;

        // In-memory "client": run_proxy READS client_rx (client→gateway) and
        // WRITES client_tx (gateway→client).
        let (client_in_tx, client_in_rx) =
            futures_channel::mpsc::unbounded::<Result<AxMessage, axum::Error>>();
        let (client_out_tx, mut client_out_rx) = futures_channel::mpsc::unbounded::<AxMessage>();

        let count = std::sync::Arc::new(AtomicUsize::new(0));
        let tap: std::sync::Arc<dyn crate::recorder::ChannelTap> = std::sync::Arc::new(CountTap {
            count: count.clone(),
        });

        let h = tokio::spawn(async move {
            run_proxy(client_out_tx, client_in_rx, &gateway, "k1", tap).await
        });

        // Send a text + a binary frame "from the client".
        client_in_tx
            .unbounded_send(Ok(AxMessage::Text("hello".into())))
            .unwrap();
        client_in_tx
            .unbounded_send(Ok(AxMessage::Binary(vec![1u8, 2, 3].into())))
            .unwrap();

        // Echo bounces them back → arrive on the client_out side.
        let m1 = client_out_rx.next().await.unwrap();
        let m2 = client_out_rx.next().await.unwrap();
        assert!(
            matches!(&m1, AxMessage::Text(t) if t.as_str() == "hello"),
            "got {m1:?}"
        );
        assert!(
            matches!(&m2, AxMessage::Binary(b) if b.as_ref() == [1,2,3]),
            "got {m2:?}"
        );

        // Activity fired per forwarded message: 2 (client→gw) + 2 (gw→client) = 4.
        assert!(
            count.load(Ordering::SeqCst) >= 4,
            "activity = {}",
            count.load(Ordering::SeqCst)
        );

        // Closing the client side ends the proxy cleanly: dropping the inbound
        // sender makes client_rx yield None, so the proxy must forward a Close
        // to the gateway and finish within the timeout.
        drop(client_in_tx);
        let joined = tokio::time::timeout(std::time::Duration::from_secs(2), h).await;
        assert!(
            joined.is_ok(),
            "proxy task did not finish after client close"
        );
    }
}
