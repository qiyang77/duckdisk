use std::{future::Future, time::Duration};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MUTATION_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(100);

pub fn client_builder() -> reqwest::ClientBuilder {
    client_builder_with_timeout(REQUEST_TIMEOUT)
}

fn client_builder_with_timeout(timeout: Duration) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(timeout)
}

pub fn http_client() -> Result<reqwest::Client, String> {
    client_builder()
        .build()
        .map_err(|error| format!("Could not configure cloud HTTP client: {error}"))
}

/// Moves to cloud trash may finish remotely even if the response is lost.
/// Give them a separate, longer window; cancellation of a scan never drops
/// one of these operations.
pub fn mutation_client() -> Result<reqwest::Client, String> {
    client_builder_with_timeout(MUTATION_REQUEST_TIMEOUT)
        .build()
        .map_err(|error| format!("Could not configure cloud mutation HTTP client: {error}"))
}

pub fn uncertain_mutation_timeout(provider: &str) -> String {
    format!("{provider} did not confirm the move before the timeout. It may already have completed remotely; refresh the scan before retrying.")
}

/// Drop the whole scan future on cancellation, including any pending response,
/// response-body read, token refresh, or retry backoff. Polling keeps the existing
/// provider/account cancellation ownership without adding a second registry.
pub async fn cancellable<T>(
    future: impl Future<Output = Result<T, String>>,
    check_active: impl Fn() -> Result<(), String>,
) -> Result<T, String> {
    let mut future = Box::pin(future);
    loop {
        check_active()?;
        match tokio::time::timeout(CANCEL_POLL_INTERVAL, future.as_mut()).await {
            Ok(result) => {
                check_active()?;
                return result;
            }
            Err(_) => continue,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        pin::Pin,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        task::{Context, Poll},
        time::Instant,
    };

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn request_timeout_includes_stalled_body() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = [0; 1024];
            socket.read(&mut request).unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n")
                .unwrap();
            std::thread::sleep(Duration::from_millis(400));
        });
        runtime().block_on(async {
            let client = client_builder()
                .timeout(Duration::from_millis(100))
                .build()
                .unwrap();
            let response = client
                .get(format!("http://{address}/"))
                .send()
                .await
                .unwrap();
            assert!(response.text().await.unwrap_err().is_timeout());
        });
        server.join().unwrap();
    }

    struct PendingScan(Arc<AtomicBool>);
    impl Future for PendingScan {
        type Output = Result<(), String>;
        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
            Poll::Pending
        }
    }
    impl Drop for PendingScan {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn cancellation_drops_pending_scan_promptly() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let trigger = Arc::clone(&cancelled);
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            trigger.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let result = runtime().block_on(cancellable(PendingScan(Arc::clone(&dropped)), || {
            if cancelled.load(Ordering::SeqCst) {
                Err("cancelled".into())
            } else {
                Ok(())
            }
        }));
        thread.join().unwrap();
        assert_eq!(result.unwrap_err(), "cancelled");
        assert!(dropped.load(Ordering::SeqCst));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn cancellation_guard_preserves_success_and_error() {
        runtime().block_on(async {
            assert_eq!(cancellable(async { Ok(7) }, || Ok(())).await.unwrap(), 7);
            assert_eq!(
                cancellable(async { Err::<(), _>("failure".into()) }, || Ok(()))
                    .await
                    .unwrap_err(),
                "failure"
            );
        });
    }

    #[test]
    fn mutations_have_a_separate_timeout_and_clear_uncertain_outcome() {
        assert!(MUTATION_REQUEST_TIMEOUT > REQUEST_TIMEOUT);
        assert!(uncertain_mutation_timeout("OneDrive").contains("may already have completed"));
    }
}
