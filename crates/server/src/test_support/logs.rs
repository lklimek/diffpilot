//! Log capture for tests. One process-global subscriber is installed once (thread-scoped subscribers miss events
//! in parallel tests because callsite interest is cached process-wide); it routes every line to the buffer of
//! the thread that logged it, so parallel tests never see each other's lines. Tests run on their own thread
//! (`#[tokio::test]` uses a current-thread runtime), so a test sees all of its own lines.

use std::cell::RefCell;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use tracing_subscriber::layer::SubscriberExt;

thread_local! {
    static BUFFER: RefCell<Option<Arc<Mutex<Vec<u8>>>>> = const { RefCell::new(None) };
}

/// Lines logged on the current thread since [`LogCapture::start`].
#[derive(Debug)]
pub(crate) struct LogCapture(Arc<Mutex<Vec<u8>>>);

impl LogCapture {
    /// Installs the global subscriber on first use and starts a fresh buffer for the current thread.
    pub(crate) fn start() -> Self {
        static INSTALLED: OnceLock<()> = OnceLock::new();
        INSTALLED.get_or_init(|| {
            let _ = tracing::subscriber::set_global_default(subscriber());
        });
        Self::begin_capture()
    }

    /// Captures on this thread even when another test has already installed a global subscriber.
    pub(crate) fn start_scoped() -> (Self, tracing::subscriber::DefaultGuard) {
        let guard = tracing::subscriber::set_default(subscriber());
        (Self::begin_capture(), guard)
    }

    fn begin_capture() -> Self {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        BUFFER.with(|current| *current.borrow_mut() = Some(Arc::clone(&buffer)));
        Self(buffer)
    }

    /// Everything captured so far.
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap_or_else(PoisonError::into_inner)).into_owned()
    }
}

fn subscriber() -> impl tracing::Subscriber + Send + Sync {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(|| ThreadWriter)
        .finish()
        .with(crate::memcan::payload_log_cap())
}

impl Drop for LogCapture {
    fn drop(&mut self) {
        BUFFER.with(|current| current.borrow_mut().take());
    }
}

/// Appends to the current thread's buffer; lines of threads without a capture are dropped.
struct ThreadWriter;

impl std::io::Write for ThreadWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        BUFFER.with(|current| {
            if let Some(buffer) = current.borrow().as_ref() {
                buffer
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .extend_from_slice(data);
            }
        });
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_thread_captures_only_its_own_lines() {
        let capture = LogCapture::start();
        let other = std::thread::spawn(|| {
            let capture = LogCapture::start();
            tracing::info!("line-of-the-other-thread");
            capture.text()
        })
        .join()
        .expect("thread");
        tracing::info!("line-of-this-thread");
        let own = capture.text();
        assert!(own.contains("line-of-this-thread"), "{own}");
        assert!(!own.contains("line-of-the-other-thread"), "{own}");
        assert!(other.contains("line-of-the-other-thread"), "{other}");
        assert!(!other.contains("line-of-this-thread"), "{other}");
    }
}
