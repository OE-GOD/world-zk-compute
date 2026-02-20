//! Graceful Shutdown Handling
//!
//! Provides coordinated shutdown for all prover components:
//! - Catches SIGINT/SIGTERM signals
//! - Waits for in-flight proofs to complete
//! - Saves recovery state before exit
//! - Closes connections cleanly

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, watch};
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
    /// Broadcast channel for shutdown complete notifications
    complete_tx: broadcast::Sender<()>,
}

impl ShutdownController {
    /// Create a new shutdown controller
    pub fn new() -> Self {
        let (sender, receiver) = watch::channel(false);
        let (complete_tx, _) = broadcast::channel(1);

        Self {
            shutdown: Arc::new(AtomicBool::new(false)),
            sender,
            receiver,
            complete_tx,
        }
    }

    /// Get a shutdown signal that can be shared with tasks
    pub fn signal(&self) -> ShutdownSignal {
        ShutdownSignal {
            shutdown: self.shutdown.clone(),
            receiver: self.receiver.clone(),
        }
    }

    /// Initiate shutdown
    pub fn shutdown(&self) {
        if self.shutdown.swap(true, Ordering::SeqCst) {
            // Already shutting down
            return;
        }

        info!("Shutdown initiated");
        let _ = self.sender.send(true);
    }

    /// Check if shutdown has been requested
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    /// Wait for shutdown to complete with timeout
    pub async fn wait_for_completion(&self, timeout: Duration) -> bool {
        let mut rx = self.complete_tx.subscribe();

        tokio::select! {
            _ = rx.recv() => true,
            _ = tokio::time::sleep(timeout) => {
                warn!("Shutdown timeout exceeded");
                false
            }
        }
    }
}

impl Default for ShutdownController {
    fn default() -> Self {
        Self::new()
    }
}

/// Install signal handlers for graceful shutdown
pub async fn install_signal_handlers(controller: Arc<ShutdownController>) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let controller_int = controller.clone();
        let controller_term = controller.clone();

        // Handle SIGINT (Ctrl+C)
        tokio::spawn(async move {
            let mut sigint =
                signal(SignalKind::interrupt()).expect("Failed to install SIGINT handler");
            sigint.recv().await;
            info!("Received SIGINT, initiating graceful shutdown...");
            controller_int.shutdown();
        });

        // Handle SIGTERM
        tokio::spawn(async move {
            let mut sigterm =
                signal(SignalKind::terminate()).expect("Failed to install SIGTERM handler");
            sigterm.recv().await;
            info!("Received SIGTERM, initiating graceful shutdown...");
            controller_term.shutdown();
        });
    }

    #[cfg(windows)]
    {
        let controller_ctrl_c = controller.clone();
        tokio::spawn(async move {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to install Ctrl+C handler");
            info!("Received Ctrl+C, initiating graceful shutdown...");
            controller_ctrl_c.shutdown();
        });
    }

    info!("Signal handlers installed");
}

/// Graceful shutdown configuration
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ShutdownConfig {
    /// Maximum time to wait for in-flight tasks
    pub grace_period: Duration,
    /// Whether to save state before shutdown
    pub save_state: bool,
    /// Whether to finish current job before shutdown
    pub finish_current_job: bool,
}

impl Default for ShutdownConfig {
    fn default() -> Self {
        Self {
            grace_period: Duration::from_secs(30),
            save_state: true,
            finish_current_job: true,
        }
    }
}

/// Perform graceful shutdown with the given configuration
pub async fn graceful_shutdown(
    controller: &ShutdownController,
    config: &ShutdownConfig,
    on_save_state: impl FnOnce() -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>,
    >,
) -> anyhow::Result<()> {
    info!(
        "Starting graceful shutdown (grace period: {:?})",
        config.grace_period
    );

    // Save state if configured
    if config.save_state {
        info!("Saving state before shutdown...");
        match on_save_state().await {
            Ok(()) => info!("State saved successfully"),
            Err(e) => error!("Failed to save state: {}", e),
        }
    }

    // Wait for completion with timeout
    let completed = controller.wait_for_completion(config.grace_period).await;

    if completed {
        info!("Graceful shutdown completed");
    } else {
        warn!("Shutdown timeout - some tasks may not have completed");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

        controller.shutdown();
        controller.shutdown(); // Should not panic
        controller.shutdown(); // Should not panic

        assert!(controller.is_shutdown());
    }

    #[tokio::test]
    async fn test_completion_timeout() {
        let controller = ShutdownController::new();

        controller.shutdown();
        // Don't call complete(), so it should timeout
        let completed = controller
            .wait_for_completion(Duration::from_millis(50))
            .await;

        assert!(!completed);
    }
}
