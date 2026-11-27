//! Graceful Shutdown Handling
//!
//! Lets the prover stop between two cycles of its main loop:
//! - The first SIGINT/SIGTERM notifies the main loop, which ends after the
//!   cycle that is running
//! - A second signal exits at once

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{error, info, warn};

/// Shutdown signal that can be cloned and shared
#[derive(Clone)]
#[allow(dead_code)]
pub struct ShutdownSignal {
    /// Whether shutdown has been initiated
    shutdown: Arc<AtomicBool>,
    /// Receiver for shutdown notification
    receiver: watch::Receiver<bool>,
}

impl ShutdownSignal {
    /// Wait for shutdown signal
    pub async fn wait(&mut self) {
        // If already shutdown, return immediately
        if *self.receiver.borrow() {
            return;
        }
        // Wait for the value to change to true
        let _ = self.receiver.wait_for(|&v| v).await;
    }
}

/// Shutdown controller that manages the shutdown process
pub struct ShutdownController {
    /// Atomic flag for quick checks
    shutdown: Arc<AtomicBool>,
    /// Watch sender to notify all receivers
    sender: watch::Sender<bool>,
    /// Receiver template for creating signals
    receiver: watch::Receiver<bool>,
}

impl ShutdownController {
    /// Create a new shutdown controller
    pub fn new() -> Self {
        let (sender, receiver) = watch::channel(false);

        Self {
            shutdown: Arc::new(AtomicBool::new(false)),
            sender,
            receiver,
        }
    }

    /// Get a shutdown signal that can be shared with tasks
    pub fn signal(&self) -> ShutdownSignal {
        ShutdownSignal {
            shutdown: self.shutdown.clone(),
            receiver: self.receiver.clone(),
        }
    }

    /// Initiate shutdown; false if it was initiated before
    pub fn shutdown(&self) -> bool {
        if self.shutdown.swap(true, Ordering::SeqCst) {
            // Already shutting down
            return false;
        }

        info!("Shutdown initiated");
        let _ = self.sender.send(true);
        true
    }

    /// Check if shutdown has been requested
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }
}

impl Default for ShutdownController {
    fn default() -> Self {
        Self::new()
    }
}

/// Act on a received signal: the first one initiates shutdown, any further
/// one exits the process with `exit_code`.
fn handle_signal(controller: &ShutdownController, name: &str, exit_code: i32) {
    info!("Received {}", name);
    if !controller.shutdown() {
        warn!("Already shutting down, exiting at once");
        std::process::exit(exit_code);
    }
}

/// Install signal handlers for graceful shutdown
pub async fn install_signal_handlers(controller: Arc<ShutdownController>) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        // Exit codes as of a process killed by the signal (128 + number)
        let signals = [
            (SignalKind::interrupt(), "SIGINT (Ctrl+C)", 130),
            (SignalKind::terminate(), "SIGTERM", 143),
        ];
        for (kind, name, exit_code) in signals {
            let mut stream = match signal(kind) {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to install {} handler: {}", name, e);
                    continue;
                }
            };
            let controller = controller.clone();
            tokio::spawn(async move {
                while stream.recv().await.is_some() {
                    handle_signal(&controller, name, exit_code);
                }
            });
        }
    }

    #[cfg(windows)]
    {
        tokio::spawn(async move {
            loop {
                if let Err(e) = tokio::signal::ctrl_c().await {
                    error!("Failed to install Ctrl+C handler: {}", e);
                    return;
                }
                handle_signal(&controller, "Ctrl+C", 130);
            }
        });
    }

    info!("Signal handlers installed");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_shutdown_signal() {
        let controller = ShutdownController::new();
        let _signal = controller.signal();

        assert!(!controller.is_shutdown());

        controller.shutdown();

        assert!(controller.is_shutdown());
    }

    #[tokio::test]
    async fn test_shutdown_wait() {
        let controller = ShutdownController::new();
        let mut signal = controller.signal();

        let controller_clone = Arc::new(controller);
        let controller_for_task = controller_clone.clone();

        // Spawn task that triggers shutdown after delay
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            controller_for_task.shutdown();
        });

        // Wait should complete when shutdown is triggered
        signal.wait().await;

        assert!(controller_clone.is_shutdown());
    }

    #[tokio::test]
    async fn test_shutdown_idempotent() {
        let controller = ShutdownController::new();

        // Only the first call initiates the shutdown
        assert!(controller.shutdown());
        assert!(!controller.shutdown()); // Should not panic
        assert!(!controller.shutdown()); // Should not panic

        assert!(controller.is_shutdown());
    }
}
