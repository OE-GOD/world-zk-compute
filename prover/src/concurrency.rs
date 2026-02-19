//! GPU-Aware Concurrency Control
//!
//! Separates GPU and CPU concurrency pools to prevent small CPU-bound jobs
//! from starving GPU proving slots, and vice versa.

use crate::gpu_manager::GpuDeviceManager;
use std::sync::Arc;
use tokio::sync::{Semaphore, OwnedSemaphorePermit};
use tracing::{debug, info};

/// Resource type for a proving job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobResource {
    /// Job should use a GPU device
    Gpu,
    /// Job should use CPU only
    Cpu,
}

/// Guard returned by `ConcurrencyManager::acquire`.
///
/// Holds either a GPU device guard or a CPU semaphore permit,
/// plus the overall total-limit permit.
#[allow(dead_code)]
pub struct ConcurrencyGuard {
    /// Which resource pool was used
    pub resource: JobResource,
    /// GPU device guard (if GPU job) — sets CUDA_VISIBLE_DEVICES
    pub gpu_guard: Option<crate::gpu_manager::GpuDeviceGuard>,
    /// CPU semaphore permit (if CPU job)
    _cpu_permit: Option<OwnedSemaphorePermit>,
    /// Total concurrency permit
    _total_permit: OwnedSemaphorePermit,
}

#[allow(dead_code)]
impl ConcurrencyGuard {
    /// Get the assigned GPU device ID (if this is a GPU job).
    pub fn gpu_device_id(&self) -> Option<usize> {
        self.gpu_guard.as_ref().map(|g| g.device_id())
    }
}

/// GPU-aware concurrency manager.
///
/// Maintains separate pools for GPU and CPU jobs with an overall limit:
/// - GPU pool: managed by `GpuDeviceManager` (1 job per GPU device)
/// - CPU pool: limited to `max_cpu_jobs` concurrent jobs
/// - Total pool: overall limit prevents system overload
#[allow(dead_code)]
pub struct ConcurrencyManager {
    gpu_manager: Arc<GpuDeviceManager>,
    cpu_semaphore: Arc<Semaphore>,
    total_semaphore: Arc<Semaphore>,
    max_cpu_jobs: usize,
}

#[allow(dead_code)]
impl ConcurrencyManager {
    /// Create a new concurrency manager.
    ///
    /// - `gpu_manager`: handles GPU device assignment
    /// - `max_cpu_jobs`: max concurrent CPU-only proving jobs (0 = auto: num_cpus - 1)
    pub fn new(gpu_manager: Arc<GpuDeviceManager>, max_cpu_jobs: usize) -> Self {
        let cpu_jobs = if max_cpu_jobs == 0 {
            // Auto-detect: leave one CPU for async runtime
            std::thread::available_parallelism()
                .map(|p| p.get().saturating_sub(1).max(1))
                .unwrap_or(3)
        } else {
            max_cpu_jobs
        };

        let gpu_count = gpu_manager.device_count();
        let total = gpu_count + cpu_jobs;

        info!(
            "ConcurrencyManager: {} GPU slot(s), {} CPU slot(s), {} total",
            gpu_count, cpu_jobs, total
        );

        Self {
            gpu_manager,
            cpu_semaphore: Arc::new(Semaphore::new(cpu_jobs)),
            total_semaphore: Arc::new(Semaphore::new(total)),
            max_cpu_jobs: cpu_jobs,
        }
    }

    /// Classify a job based on preflight results.
    ///
    /// Jobs with few cycles or high memory use are better on CPU.
    /// Larger proving jobs benefit from GPU acceleration.
    pub fn classify_job(&self, cycles: u64, memory_bytes: usize) -> JobResource {
        // If no GPU is available, always use CPU
        if !self.gpu_manager.has_gpu() {
            return JobResource::Cpu;
        }

        let memory_mb = memory_bytes / (1024 * 1024);

        // Small programs (<5M cycles) or very memory-heavy programs use CPU
        if cycles < 5_000_000 || memory_mb > 512 {
            JobResource::Cpu
        } else {
            JobResource::Gpu
        }
    }

    /// Acquire a concurrency slot for the given resource type.
    ///
    /// For GPU jobs: acquires a GPU device (with CUDA_VISIBLE_DEVICES set)
    /// and a total-limit permit.
    ///
    /// For CPU jobs: acquires a CPU semaphore permit and a total-limit permit.
    ///
    /// If GPU acquisition fails (e.g. no devices), falls back to CPU.
    pub async fn acquire(&self, resource: JobResource) -> anyhow::Result<ConcurrencyGuard> {
        // Acquire total limit first
        let total_permit = self
            .total_semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| anyhow::anyhow!("Total concurrency semaphore closed"))?;

        match resource {
            JobResource::Gpu => {
                if let Some(gpu_guard) = self.gpu_manager.acquire_device().await {
                    debug!("Acquired GPU device {} for job", gpu_guard.device_id());
                    Ok(ConcurrencyGuard {
                        resource: JobResource::Gpu,
                        gpu_guard: Some(gpu_guard),
                        _cpu_permit: None,
                        _total_permit: total_permit,
                    })
                } else {
                    // GPU unavailable, fall back to CPU
                    debug!("GPU unavailable, falling back to CPU");
                    let cpu_permit = self
                        .cpu_semaphore
                        .clone()
                        .acquire_owned()
                        .await
                        .map_err(|_| anyhow::anyhow!("CPU semaphore closed"))?;

                    Ok(ConcurrencyGuard {
                        resource: JobResource::Cpu,
                        gpu_guard: None,
                        _cpu_permit: Some(cpu_permit),
                        _total_permit: total_permit,
                    })
                }
            }
            JobResource::Cpu => {
                let cpu_permit = self
                    .cpu_semaphore
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|_| anyhow::anyhow!("CPU semaphore closed"))?;

                debug!("Acquired CPU slot for job");

                Ok(ConcurrencyGuard {
                    resource: JobResource::Cpu,
                    gpu_guard: None,
                    _cpu_permit: Some(cpu_permit),
                    _total_permit: total_permit,
                })
            }
        }
    }

    /// Get the underlying GPU device manager.
    pub fn gpu_manager(&self) -> &Arc<GpuDeviceManager> {
        &self.gpu_manager
    }

    /// Maximum CPU concurrency slots.
    pub fn max_cpu_jobs(&self) -> usize {
        self.max_cpu_jobs
    }

    /// Available CPU slots.
    pub fn available_cpu_slots(&self) -> usize {
        self.cpu_semaphore.available_permits()
    }

    /// Available total slots.
    pub fn available_total_slots(&self) -> usize {
        self.total_semaphore.available_permits()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu_optimize::GpuBackend;

    #[test]
    fn test_classify_small_job_as_cpu() {
        let gpu_mgr = Arc::new(GpuDeviceManager::with_device_count(GpuBackend::Cuda, 2));
        let mgr = ConcurrencyManager::new(gpu_mgr, 2);

        // Small program → CPU
        assert_eq!(mgr.classify_job(1_000_000, 32 * 1024 * 1024), JobResource::Cpu);
    }

    #[test]
    fn test_classify_large_job_as_gpu() {
        let gpu_mgr = Arc::new(GpuDeviceManager::with_device_count(GpuBackend::Cuda, 2));
        let mgr = ConcurrencyManager::new(gpu_mgr, 2);

        // Large program → GPU
        assert_eq!(mgr.classify_job(50_000_000, 128 * 1024 * 1024), JobResource::Gpu);
    }

    #[test]
    fn test_classify_without_gpu() {
        let gpu_mgr = Arc::new(GpuDeviceManager::with_device_count(GpuBackend::Cpu, 0));
        let mgr = ConcurrencyManager::new(gpu_mgr, 2);

        // No GPU → always CPU
        assert_eq!(mgr.classify_job(50_000_000, 128 * 1024 * 1024), JobResource::Cpu);
    }

    #[tokio::test]
    async fn test_acquire_cpu_slot() {
        let gpu_mgr = Arc::new(GpuDeviceManager::with_device_count(GpuBackend::Cpu, 0));
        let mgr = ConcurrencyManager::new(gpu_mgr, 2);

        let guard = mgr.acquire(JobResource::Cpu).await.unwrap();
        assert_eq!(guard.resource, JobResource::Cpu);
        assert!(guard.gpu_device_id().is_none());
    }

    #[tokio::test]
    async fn test_acquire_gpu_slot() {
        let gpu_mgr = Arc::new(GpuDeviceManager::with_device_count(GpuBackend::Cuda, 2));
        let mgr = ConcurrencyManager::new(gpu_mgr, 2);

        let guard = mgr.acquire(JobResource::Gpu).await.unwrap();
        assert_eq!(guard.resource, JobResource::Gpu);
        assert!(guard.gpu_device_id().is_some());
    }

    #[tokio::test]
    async fn test_gpu_fallback_to_cpu() {
        // No GPU devices → GPU acquire falls back to CPU
        let gpu_mgr = Arc::new(GpuDeviceManager::with_device_count(GpuBackend::Cpu, 0));
        let mgr = ConcurrencyManager::new(gpu_mgr, 2);

        let guard = mgr.acquire(JobResource::Gpu).await.unwrap();
        assert_eq!(guard.resource, JobResource::Cpu);
    }
}
