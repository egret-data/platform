//! #787: the boot repair of role-referencing profiles retries a failure, off
//! the boot path, with the backoff the owner chose (5 s, 30 s, 2 min), and
//! stops after the last retry. Time is paused, so the waits cost nothing and
//! are measured exactly.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use egret_perm::CompileError;
use egret_perm_compile::role_membership::{retry_boot_repair, BOOT_REPAIR_BACKOFF};

fn failure() -> CompileError {
    CompileError::from_db(sqlx::Error::PoolTimedOut)
}

#[test]
fn the_backoff_is_the_one_the_owner_chose() {
    assert_eq!(
        BOOT_REPAIR_BACKOFF,
        [
            Duration::from_secs(5),
            Duration::from_secs(30),
            Duration::from_secs(120)
        ]
    );
}

/// Arrange: an attempt that fails twice, then succeeds.
/// Act: run the retry loop.
/// Assert: three attempts, and exactly 5 s + 30 s waited between them.
#[tokio::test(start_paused = true)]
async fn a_failed_repair_is_retried_after_each_wait_until_it_succeeds() {
    let calls = AtomicUsize::new(0);
    let started = tokio::time::Instant::now();

    let out = retry_boot_repair(&BOOT_REPAIR_BACKOFF, || {
        let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
        async move {
            if n <= 2 {
                Err(failure())
            } else {
                Ok(vec![7])
            }
        }
    })
    .await;

    assert_eq!(out, Some(vec![7]), "the third attempt succeeded");
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(started.elapsed(), Duration::from_secs(5 + 30));
}

/// Arrange: an attempt that always fails.
/// Act: run the retry loop.
/// Assert: four attempts (one plus three retries), every wait taken, then it
/// gives up rather than looping.
#[tokio::test(start_paused = true)]
async fn a_repair_that_keeps_failing_gives_up_after_the_last_retry() {
    let calls = AtomicUsize::new(0);
    let started = tokio::time::Instant::now();

    let out = retry_boot_repair(&BOOT_REPAIR_BACKOFF, || {
        calls.fetch_add(1, Ordering::SeqCst);
        async { Err(failure()) }
    })
    .await;

    assert_eq!(out, None);
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    assert_eq!(started.elapsed(), Duration::from_secs(5 + 30 + 120));
}

/// A first-try success waits for nothing.
#[tokio::test(start_paused = true)]
async fn a_repair_that_succeeds_at_once_is_not_retried() {
    let calls = AtomicUsize::new(0);
    let started = tokio::time::Instant::now();
    let out = retry_boot_repair(&BOOT_REPAIR_BACKOFF, || {
        calls.fetch_add(1, Ordering::SeqCst);
        async { Ok(Vec::new()) }
    })
    .await;
    assert_eq!(out, Some(Vec::new()));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(started.elapsed(), Duration::ZERO);
}
