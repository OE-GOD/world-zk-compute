//! Multi-GPU Device Manager
//!
//! Discovers available GPUs and distributes proving jobs across them
//! with per-device concurrency control.
//!
//! - **CUDA**: Enumerates NVIDIA GPUs via `nvidia-smi`
//! - **Metal**: Single device (Apple Silicon unified memory)
//! - **CPU**: No GPU devices, falls back to CPU pool

use crate::gpu_optimize::GpuBackend;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::{debug, info, warn};

/// A single GPU device with its own concurrency semaphore.
pub struct GpuDevice {
    /// Device index (0, 1, 2, ...)
    pub id: usize,
    /// Device name (e.g. "NVIDIA A100")
    pub name: String,
    /// VRAM in bytes
    pub memory_bytes: u64,
    /// Per-device semaphore (1 proving job per GPU)
    pub semaphore: Arc<Semaphore>,
}

/// Per-device utilization stats for metrics.
#[derive(Debug, Clone)]
pub struct GpuDeviceStats {
    pub id: usize,
    pub name: String,
    pub memory_bytes: u64,
    /// Number of available permits (0 = busy)
    pub available: usize,
}

/// Guard that holds a GPU device permit and sets `CUDA_VISIBLE_DEVICES`
/// for the duration of a proving job.
pub struct GpuDeviceGuard {
    /// The device index assigned to this job
    pub device_id: usize,
    /// Previous value of CUDA_VISIBLE_DEVICES (restored on drop)
    previous_cuda_visible: Option<String>,
    /// Whether CUDA_VISIBLE_DEVICES was originally set
    was_set: bool,
    /// Held semaphore permit (released on drop)
    _permit: OwnedSemaphorePermit,
}

impl GpuDeviceGuard {
    /// Get the assigned device ID.
    pub fn device_id(&self) -> usize {
        self.device_id
    }
}

impl Drop for GpuDeviceGuard {
    fn drop(&mut self) {
        // Restore original CUDA_VISIBLE_DEVICES
        if self.was_set {
            if let Some(ref prev) = self.previous_cuda_visible {
                std::env::set_var("CUDA_VISIBLE_DEVICES", prev);
            }
        } else {
            std::env::remove_var("CUDA_VISIBLE_DEVICES");
        }
        debug!("Released GPU device {}", self.device_id);
    }
}

/// Multi-GPU device manager.
///
/// Discovers GPUs at startup and distributes jobs across them
/// using round-robin assignment with per-device semaphores.
pub struct GpuDeviceManager {
    devices: Vec<GpuDevice>,
    next_device: AtomicUsize,
    backend: GpuBackend,
}

impl GpuDeviceManager {
    /// Detect available GPUs and create manager.
    ///
    /// - **CUDA**: Parses `nvidia-smi` output to enumerate devices
    /// - **Metal**: Creates a single device entry
    /// - **CPU**: No devices (GPU acquire will fail gracefully)
    pub fn detect() -> Self {
        let backend = GpuBackend::detect();

        let devices = match backend {
            GpuBackend::Cuda => Self::detect_cuda_devices(),
            GpuBackend::Metal => Self::detect_metal_devices(),
            GpuBackend::Cpu => vec![],
        };

        if devices.is_empty() && backend.is_gpu() {
            warn!("GPU backend {} detected but no devices enumerated", backend);
        } else {
            info!(
                "GPU manager initialized: {} backend, {} device(s)",
                backend,
                devices.len()
            );
            for dev in &devices {
                info!(
                    "  GPU {}: {} ({:.0} MB VRAM)",
                    dev.id,
                    dev.name,
                    dev.memory_bytes as f64 / 1024.0 / 1024.0
                );
            }
        }

        Self {
            devices,
            next_device: AtomicUsize::new(0),
            backend,
        }
    }

    /// Create a manager with a specific backend and device count.
    ///
    /// Useful for overriding auto-detection (e.g. `--max-gpu-concurrent`).
    pub fn with_device_count(backend: GpuBackend, count: usize) -> Self {
        let devices: Vec<GpuDevice> = (0..count)
            .map(|i| GpuDevice {
                id: i,
                name: format!("{} device {}", backend, i),
                memory_bytes: 0,
                semaphore: Arc::new(Semaphore::new(1)),
            })
            .collect();

        info!(
            "GPU manager initialized (manual): {} backend, {} device(s)",
            backend,
            devices.len()
        );

        Self {
            devices,
            next_device: AtomicUsize::new(0),
            backend,
        }
    }

    /// Detect NVIDIA GPUs using nvidia-smi.
    fn detect_cuda_devices() -> Vec<GpuDevice> {
        let output = std::process::Command::new("nvidia-smi")
            .args([
                "--query-gpu=index,name,memory.total",
                "--format=csv,noheader,nounits",
            ])
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let mut devices = Vec::new();

                for line in stdout.lines() {
                    let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
                    if parts.len() >= 3 {
                        let id = parts[0].parse::<usize>().unwrap_or(devices.len());
                        let name = parts[1].to_string();
                        // nvidia-smi reports memory in MiB
                        let memory_mib = parts[2].parse::<u64>().unwrap_or(0);
                        let memory_bytes = memory_mib * 1024 * 1024;

                        devices.push(GpuDevice {
                            id,
                            name,
                            memory_bytes,
                            semaphore: Arc::new(Semaphore::new(1)),
                        });
                    }
                }

                if devices.is_empty() {
                    warn!("nvidia-smi returned no devices");
                }

                devices
            }
            Ok(out) => {
                warn!(
                    "nvidia-smi failed with status {}: {}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr)
                );
                // Fall back to single device assumption
                vec![GpuDevice {
                    id: 0,
                    name: "NVIDIA GPU (unknown)".to_string(),
                    memory_bytes: 0,
                    semaphore: Arc::new(Semaphore::new(1)),
                }]
            }
            Err(e) => {
                warn!("Failed to run nvidia-smi: {}", e);
                // Fall back to single device assumption when CUDA feature is enabled
                vec![GpuDevice {
                    id: 0,
                    name: "NVIDIA GPU (nvidia-smi unavailable)".to_string(),
                    memory_bytes: 0,
                    semaphore: Arc::new(Semaphore::new(1)),
                }]
            }
        }
    }

    /// Detect Metal GPU (Apple Silicon — single unified memory device).
    fn detect_metal_devices() -> Vec<GpuDevice> {
        vec![GpuDevice {
            id: 0,
            name: "Apple Metal GPU".to_string(),
            // Apple Silicon has unified memory; report system RAM as a proxy
            memory_bytes: Self::get_system_memory().unwrap_or(16 * 1024 * 1024 * 1024),
            semaphore: Arc::new(Semaphore::new(1)),
        }]
    }

    /// Get system memory (used as VRAM proxy for unified memory architectures).
    fn get_system_memory() -> Option<u64> {
        #[cfg(target_os = "macos")]
        {
            let output = std::process::Command::new("sysctl")
                .args(["-n", "hw.memsize"])
                .output()
                .ok()?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout.trim().parse::<u64>().ok()
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }

    /// Acquire a GPU device for a proving job (round-robin).
    ///
    /// Returns a guard that:
    /// 1. Sets `CUDA_VISIBLE_DEVICES=N` for the assigned device
    /// 2. Holds the per-device semaphore permit
    /// 3. Restores the environment and releases the permit on drop
    ///
    /// Returns `None` if no GPU devices are available.
    pub async fn acquire_device(&self) -> Option<GpuDeviceGuard> {
        if self.devices.is_empty() {
            return None;
        }

        // Round-robin select next device
        let idx = self.next_device.fetch_add(1, Ordering::Relaxed) % self.devices.len();
        let device = &self.devices[idx];

        debug!("Waiting for GPU device {} ({})", device.id, device.name);

        // Acquire permit (blocks until device is free)
        let permit = device.semaphore.clone().acquire_owned().await.ok()?;

        // Save and set CUDA_VISIBLE_DEVICES
        let was_set = std::env::var("CUDA_VISIBLE_DEVICES").is_ok();
        let previous = std::env::var("CUDA_VISIBLE_DEVICES").ok();
        std::env::set_var("CUDA_VISIBLE_DEVICES", device.id.to_string());

        debug!(
            "Acquired GPU device {} ({}) — CUDA_VISIBLE_DEVICES={}",
            device.id, device.name, device.id
        );

        Some(GpuDeviceGuard {
            device_id: device.id,
            previous_cuda_visible: previous,
            was_set,
            _permit: permit,
        })
    }

    /// Number of GPU devices available.
    pub fn device_count(&self) -> usize {
        self.devices.len()
    }

    /// Get the detected GPU backend type.
    pub fn backend(&self) -> GpuBackend {
        self.backend
    }

    /// Whether any GPU is available.
    pub fn has_gpu(&self) -> bool {
        !self.devices.is_empty()
    }

    /// Per-device utilization stats for metrics.
    pub fn stats(&self) -> Vec<GpuDeviceStats> {
        self.devices
            .iter()
            .map(|dev| GpuDeviceStats {
                id: dev.id,
                name: dev.name.clone(),
                memory_bytes: dev.memory_bytes,
                available: dev.semaphore.available_permits(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_manager_has_no_devices() {
        let manager = GpuDeviceManager::with_device_count(GpuBackend::Cpu, 0);
        assert_eq!(manager.device_count(), 0);
        assert!(!manager.has_gpu());
    }

    #[test]
    fn test_manual_device_count() {
        let manager = GpuDeviceManager::with_device_count(GpuBackend::Cuda, 4);
        assert_eq!(manager.device_count(), 4);
        assert!(manager.has_gpu());
    }

    #[tokio::test]
    async fn test_round_robin_assignment() {
        let manager = GpuDeviceManager::with_device_count(GpuBackend::Cuda, 3);

        // Acquire and immediately release 3 devices
        for expected_id in 0..3 {
            let guard = manager.acquire_device().await.unwrap();
            assert_eq!(guard.device_id(), expected_id);
            drop(guard);
        }

        // Next should wrap around to device 0
        let guard = manager.acquire_device().await.unwrap();
        assert_eq!(guard.device_id(), 0);
    }

    #[test]
    fn test_stats() {
        let manager = GpuDeviceManager::with_device_count(GpuBackend::Cuda, 2);
        let stats = manager.stats();
        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].available, 1); // Not acquired yet
        assert_eq!(stats[1].available, 1);
    }

    #[tokio::test]
    async fn test_no_device_returns_none() {
        let manager = GpuDeviceManager::with_device_count(GpuBackend::Cpu, 0);
        let guard = manager.acquire_device().await;
        assert!(guard.is_none());
    }
}
