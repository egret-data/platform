//! `serve_with_drain` over a real socket.
//!
//! # What this file refuses to assert
//!
//! That a shutdown handler was *installed*. That test is green with the drain
//! deleted, because installing a handler and honouring it are different facts,
//! and this repo has shipped the first while claiming the second. So the
//! assertion here is the outcome an operator cares about: **an in-flight request
//! that was mid-flight when SIGTERM arrived received its complete response** —
//! read off a real TCP connection, byte by byte, after the shutdown was
//! triggered.
//!
//! # Why a hand-written client
//!
//! The crate has no HTTP client dependency and this needs none. A raw
//! `TcpStream` plus a `GET / HTTP/1.1` line is also stricter than a client would
//! be: a connection that is reset mid-response — the failure mode a missing
//! drain actually produces — shows up as a short read here, where a forgiving
//! client might report it as an error indistinguishable from a hundred others.

use axum::routing::get;
use axum::Router;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Send a bare HTTP/1.1 request and read until the peer closes.
///
/// `Connection: close` so "the server finished answering" and "the socket ended"
/// are the same event and no framing has to be parsed.
async fn raw_request(addr: std::net::SocketAddr, path: &str) -> std::io::Result<String> {
    let mut sock = TcpStream::connect(addr).await?;
    sock.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .await?;
    let mut buf = Vec::new();
    sock.read_to_end(&mut buf).await?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// The property the whole change exists for.
///
/// # The assertion that a first draft got wrong, recorded because it looked right
///
/// "The client read a complete response after the signal" is **not enough**, and
/// a mutation proved it: with the drain deadline set to `Duration::ZERO` the
/// test stayed green. In-process, `axum::serve` runs each connection on a
/// `tokio::spawn`ed task, so dropping the server future leaves those tasks
/// running and the response arrives regardless. In production the same moment is
/// `main` returning — the process exits and every one of those tasks dies with
/// it.
///
/// So the property that actually maps onto a rollout is **when
/// `serve_with_drain` returns**: it must not return while a request is still in
/// flight, because that return IS the process exiting. Both halves are asserted
/// below; the body check alone was the weaker one.
#[tokio::test]
async fn an_in_flight_request_completes_before_the_server_returns() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    // Set once the handler has genuinely begun, so the signal is fired while a
    // request is in flight rather than merely soon after one was sent.
    let in_handler = Arc::new(tokio::sync::Notify::new());
    let entered = in_handler.clone();
    let handler_done = Arc::new(AtomicBool::new(false));
    let done_in_handler = handler_done.clone();
    let app = Router::new().route(
        "/slow",
        get(move || {
            let entered = entered.clone();
            let done = done_in_handler.clone();
            async move {
                entered.notify_waiters();
                tokio::time::sleep(Duration::from_millis(400)).await;
                done.store(true, Ordering::SeqCst);
                "the-whole-body"
            }
        }),
    );

    let (sigterm_tx, sigterm_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(egret_server::serve_with_drain(
        listener,
        app,
        async move {
            let _ = sigterm_rx.await;
        },
        Duration::from_secs(10),
    ));

    let waiting = in_handler.notified();
    tokio::pin!(waiting);
    waiting.as_mut().enable();
    let client = tokio::spawn(async move { raw_request(addr, "/slow").await });
    waiting.await;

    // The handler is running RIGHT NOW. This is the moment a rollout would have
    // killed it.
    sigterm_tx.send(()).unwrap();

    // THE assertion. `serve_with_drain` returning is the process exiting; if it
    // returns here, the request in flight right now is one a rollout killed.
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("the server did not exit after the drain")
        .unwrap()
        .unwrap();
    assert!(
        handler_done.load(Ordering::SeqCst),
        "serve_with_drain returned while a request was still in the handler — in \
         production that return is the process exiting, so this request would \
         have been killed by the deploy"
    );

    // And the caller got the whole thing, not a truncated body.
    let response = client
        .await
        .unwrap()
        .expect("connection was reset mid-response");
    assert!(
        response.starts_with("HTTP/1.1 200 OK"),
        "in-flight request did not get its response: {response:?}"
    );
    assert!(
        response.ends_with("the-whole-body"),
        "the response was truncated by the shutdown: {response:?}"
    );
}

/// The other half: the drain is BOUNDED. A request that will never finish must
/// not hold the process open until SIGKILL, which is the same outcome as no
/// drain at all reached more slowly.
///
/// Raise the deadline to `Duration::MAX` — or drop the `select!` arm that
/// enforces it — and this test hangs until its own guard fires.
#[tokio::test]
async fn the_drain_gives_up_at_its_deadline() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let in_handler = Arc::new(tokio::sync::Notify::new());
    let entered = in_handler.clone();
    let finished = Arc::new(AtomicBool::new(false));
    let finished_in_handler = finished.clone();
    let app = Router::new().route(
        "/forever",
        get(move || {
            let entered = entered.clone();
            let finished = finished_in_handler.clone();
            async move {
                entered.notify_waiters();
                // Far beyond the drain deadline below: this request cannot be
                // what ends the wait.
                tokio::time::sleep(Duration::from_secs(3600)).await;
                finished.store(true, Ordering::SeqCst);
                "never"
            }
        }),
    );

    let (sigterm_tx, sigterm_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(egret_server::serve_with_drain(
        listener,
        app,
        async move {
            let _ = sigterm_rx.await;
        },
        Duration::from_millis(300),
    ));

    let waiting = in_handler.notified();
    tokio::pin!(waiting);
    waiting.as_mut().enable();
    let client = tokio::spawn(async move { raw_request(addr, "/forever").await });
    waiting.await;

    let started = std::time::Instant::now();
    sigterm_tx.send(()).unwrap();

    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("the drain never gave up — an unfinishable request held shutdown open")
        .unwrap()
        .unwrap();

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the drain took {:?}, which is not a bound",
        started.elapsed()
    );
    assert!(
        !finished.load(Ordering::SeqCst),
        "the handler cannot have completed; if it did, this test proved nothing \
         about the deadline"
    );
    client.abort();
}
