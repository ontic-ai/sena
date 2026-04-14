//! Async token stream for inference output.

use crate::error::InferenceError;
use tokio::sync::mpsc;

/// Async stream of tokens produced during inference.
pub struct InferenceStream {
    rx: mpsc::Receiver<Result<String, InferenceError>>,
}

impl InferenceStream {
    /// Create a new inference stream from a receiver.
    pub fn new(rx: mpsc::Receiver<Result<String, InferenceError>>) -> Self {
        Self { rx }
    }

    /// Create a channel pair for streaming tokens.
    pub fn channel(buffer: usize) -> (mpsc::Sender<Result<String, InferenceError>>, Self) {
        let (tx, rx) = mpsc::channel(buffer);
        (tx, Self::new(rx))
    }

    /// Receive the next token from the stream.
    pub async fn next(&mut self) -> Option<Result<String, InferenceError>> {
        self.rx.recv().await
    }

    /// Collect all tokens into a single string.
    pub async fn collect_all(mut self) -> Result<String, InferenceError> {
        let mut output = String::new();
        while let Some(result) = self.next().await {
            match result {
                Ok(token) => output.push_str(&token),
                Err(e) => return Err(e),
            }
        }
        Ok(output)
    }
}
