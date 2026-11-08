//! Bounded stderr capture for background processes, independent of console handles.
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio::{io::AsyncReadExt, process::ChildStderr, task::JoinHandle};

const MAX_STDERR: usize = 16 * 1024;
pub(crate) struct StderrCapture {
    tail: Arc<Mutex<VecDeque<u8>>>,
    reader: JoinHandle<()>,
}
impl StderrCapture {
    pub fn new(mut stderr: ChildStderr) -> Self {
        let tail = Arc::new(Mutex::new(VecDeque::with_capacity(MAX_STDERR)));
        let output = tail.clone();
        let reader = tokio::spawn(async move {
            let mut buffer = [0u8; 4096];
            while let Ok(count) = stderr.read(&mut buffer).await {
                if count == 0 {
                    break;
                }
                let mut tail = output
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let excess = (tail.len() + count).saturating_sub(MAX_STDERR);
                tail.drain(..excess);
                tail.extend(&buffer[..count]);
            }
        });
        Self { tail, reader }
    }
    pub fn context(&self, message: &str) -> String {
        let bytes: Vec<_> = self
            .tail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .copied()
            .collect();
        let text = String::from_utf8_lossy(&bytes);
        if text.trim().is_empty() {
            message.to_owned()
        } else {
            format!("{message}\nBackend stderr: {}", text.trim())
        }
    }
    pub async fn finish(&mut self) {
        // A descendant could keep the pipe open after its parent exits.
        let _ = tokio::time::timeout(std::time::Duration::from_millis(200), &mut self.reader).await;
    }
}
impl Drop for StderrCapture {
    fn drop(&mut self) {
        self.reader.abort();
    }
}
