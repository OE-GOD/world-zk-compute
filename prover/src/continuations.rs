//! Continuation and Segmented Proof Support
//!
//! Handles large programs that exceed single-segment limits by leveraging
//! risc0's automatic segmentation with tuned configuration:
//!
//! ## How It Works
//!
//! ```text
//! [ELF + Input] → execute() → [Session with N segments]
//!                                  ↓
//!                            prove(po2=tuned) → [N segment proofs in parallel]
//!                                  ↓
//!                          compose/compress → [Single receipt]
//!                                  ↓
//!                          (optional Groth16) → [On-chain proof]
//! ```
//!
//! risc0 handles segment-level proving internally. Our job is to:
//! 1. Choose optimal `segment_limit_po2` for parallelism
//! 2. Configure thread count to match segment count
//! 3. Detect programs that should be offloaded to Bonsai
//! 4. Track segment-level metrics for observability
//!
//! ## Staged Pipeline
//!
//! For multi-step algorithms (preprocessing → extraction → inference),
//! each stage is an independent program with its own proof. This is
//! application-level decomposition — complementary to segment-level.

#![allow(dead_code)]

use alloy::primitives::B256;
use anyhow::{anyhow, Result};
use risc0_zkvm::{default_executor, ExecutorEnv};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tracing::{info, warn};

use crate::bonsai::ProvingMode;
use crate::prover::UnifiedProver;
use crate::segment_prover::{
    estimate_segments_for_po2, optimal_po2, recommended_thread_count,
    SegmentProver, SegmentProverConfig,
};

// ═══════════════════════════════════════════════════════════════════════════════
// Configuration
// ═══════════════════════════════════════════════════════════════════════════════

/// Configuration for handling large programs.
#[derive(Clone, Debug)]
pub struct LargeProgramConfig {
    /// Maximum cycles per segment (used for estimation only; actual
    /// segmentation is controlled by `segment_limit_po2`).
    pub max_cycles_per_segment: u64,
    /// Enable proof compression (Composite → Succinct/Groth16).
    pub enable_compression: bool,
    /// Maximum segments before failing.
    pub max_segments: usize,
    /// Memory limit per segment (MB) — for estimation and warnings.
    pub memory_limit_mb: usize,
    /// Cycle threshold above which Bonsai is preferred.
    pub bonsai_threshold_cycles: u64,
}

impl Default for LargeProgramConfig {
    fn default() -> Self {
        Self {
            max_cycles_per_segment: 50_000_000,
            enable_compression: true,
            max_segments: 2000,
            memory_limit_mb: 256,
            bonsai_threshold_cycles: 100_000_000,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Execution Segment Info
// ═══════════════════════════════════════════════════════════════════════════════

/// Metadata about an execution segment (from preflight analysis).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentInfo {
    /// Segment index (0-based).
    pub index: usize,
    /// Cycles used in this segment.
    pub cycles: u64,
}

/// Result of analyzing a large program execution.
#[derive(Debug, Clone)]
pub struct ExecutionAnalysis {
    /// Total cycles across all segments.
    pub total_cycles: u64,
    /// Number of segments at the default po2.
    pub segment_count: usize,
    /// Estimated memory usage in bytes.
    pub memory_estimate_bytes: usize,
    /// Public outputs from execution.
    pub journal: Vec<u8>,
    /// Optimal segment_limit_po2 for this program.
    pub optimal_po2: u32,
    /// Estimated segments with the optimal po2.
    pub optimal_segment_count: usize,
    /// Recommended proving threads.
    pub recommended_threads: usize,
    /// Whether Bonsai is recommended for this program.
    pub recommend_bonsai: bool,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Continuation Executor
// ═══════════════════════════════════════════════════════════════════════════════

/// Handles large program execution with automatic segmentation.
///
/// Uses risc0's built-in segmentation via tuned `segment_limit_po2` and
/// parallel segment proving. For programs above the Bonsai threshold,
/// recommends cloud proving.
pub struct ContinuationExecutor {
    config: LargeProgramConfig,
}

impl ContinuationExecutor {
    /// Create a new continuation executor.
    pub fn new(config: LargeProgramConfig) -> Self {
        Self { config }
    }

    /// Analyze a program without proving — returns execution metadata
    /// and recommendations for optimal proving strategy.
    pub async fn analyze_program(
        &self,
        elf: &[u8],
        input: &[u8],
    ) -> Result<ExecutionAnalysis> {
        info!("Analyzing program for continuation support...");

        let elf_owned = elf.to_vec();
        let input_owned = input.to_vec();

        let analysis = tokio::task::spawn_blocking(move || {
            let start = Instant::now();

            let env = ExecutorEnv::builder()
                .write_slice(&input_owned)
                .build()
                .map_err(|e| anyhow!("Failed to build executor env: {}", e))?;

            let executor = default_executor();
            let session = executor
                .execute(env, &elf_owned)
                .map_err(|e| anyhow!("Execution failed: {}", e))?;

            let total_cycles = session.cycles();
            let segment_count = session.segments.len();
            let memory_estimate_bytes = segment_count * 16 * 1024 * 1024;
            let journal = session.journal.bytes.clone();

            let po2 = optimal_po2(total_cycles);
            let optimal_segment_count = estimate_segments_for_po2(total_cycles, po2);
            let recommended_threads = recommended_thread_count(optimal_segment_count);

            let elapsed = start.elapsed();
            info!(
                "Analysis complete in {:?}: {} cycles, {} segments (default), \
                 {} segments (tuned po2={}), {} threads recommended",
                elapsed, total_cycles, segment_count,
                optimal_segment_count, po2, recommended_threads
            );

            Ok::<_, anyhow::Error>(ExecutionAnalysis {
                total_cycles,
                segment_count,
                memory_estimate_bytes,
                journal,
                optimal_po2: po2,
                optimal_segment_count,
                recommended_threads,
                recommend_bonsai: false, // set below
            })
        })
        .await??;

        let mut analysis = analysis;
        analysis.recommend_bonsai =
            analysis.total_cycles > self.config.bonsai_threshold_cycles;

        if analysis.recommend_bonsai {
            warn!(
                "Program has {} cycles (> {} threshold). Bonsai recommended.",
                analysis.total_cycles,
                self.config.bonsai_threshold_cycles
            );
        }

        if analysis.optimal_segment_count > self.config.max_segments {
            return Err(anyhow!(
                "Program requires ~{} segments (limit: {}). \
                 Reduce input size or use Bonsai cloud proving.",
                analysis.optimal_segment_count,
                self.config.max_segments
            ));
        }

        Ok(analysis)
    }

    /// Execute and prove a large program using the SegmentProver.
    ///
    /// This is the primary entry point for proving programs that benefit
    /// from continuation support. It:
    /// 1. Analyzes the program (preflight)
    /// 2. Configures optimal segment size and parallelism
    /// 3. Proves with the tuned configuration
    pub async fn prove_large_program(
        &self,
        elf: &[u8],
        input: &[u8],
        proving_mode: ProvingMode,
        use_snark: bool,
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        let analysis = self.analyze_program(elf, input).await?;

        info!(
            "Proving large program: {} cycles, po2={}, threads={}, bonsai={}",
            analysis.total_cycles,
            analysis.optimal_po2,
            analysis.recommended_threads,
            analysis.recommend_bonsai,
        );

        // If Bonsai is recommended and available, offload
        if analysis.recommend_bonsai && proving_mode.uses_bonsai() {
            info!("Offloading to Bonsai cloud proving");
            let prover = UnifiedProver::new(ProvingMode::Bonsai)?;
            return prover.prove_with_snark(elf, input, use_snark).await;
        }

        // Use SegmentProver with tuned configuration
        let config = SegmentProverConfig {
            segment_limit_po2: Some(analysis.optimal_po2),
            proving_threads: analysis.recommended_threads,
            cache_executions: true,
            max_cycles: self.config.max_segments as u64 * self.config.max_cycles_per_segment,
            max_segments: self.config.max_segments,
            bonsai_threshold_cycles: self.config.bonsai_threshold_cycles,
        };

        let segment_prover = SegmentProver::new(config, proving_mode, use_snark);
        let result = segment_prover.prove_optimized(elf, input).await?;

        info!(
            "Large program proof complete: {} segments, {:?}",
            result.segment_count, result.prove_time
        );

        Ok((result.seal, result.journal))
    }

    /// Estimate segments needed for a program based on cycle count.
    pub fn estimate_segments(&self, estimated_cycles: u64) -> usize {
        let segments = (estimated_cycles / self.config.max_cycles_per_segment) as usize;
        segments.max(1)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Staged Detection Pipeline
// ═══════════════════════════════════════════════════════════════════════════════

/// Staged detection pipeline for complex multi-step algorithms.
///
/// Each stage is an independent guest program with its own image ID.
/// Stages execute sequentially — one stage's output feeds the next.
/// Each produces an independent proof that can be verified separately.
#[derive(Debug, Clone)]
pub struct StagedPipeline {
    pub stages: Vec<DetectionStage>,
}

/// A single stage in the detection pipeline.
#[derive(Debug, Clone)]
pub struct DetectionStage {
    pub name: String,
    pub image_id: B256,
    pub estimated_cycles: u64,
    pub memory_mb: usize,
}

impl StagedPipeline {
    pub fn new() -> Self {
        Self { stages: Vec::new() }
    }

    pub fn add_stage(
        mut self,
        name: &str,
        image_id: B256,
        estimated_cycles: u64,
        memory_mb: usize,
    ) -> Self {
        self.stages.push(DetectionStage {
            name: name.to_string(),
            image_id,
            estimated_cycles,
            memory_mb,
        });
        self
    }

    /// Create a standard ML detection pipeline.
    pub fn ml_detection_pipeline() -> Self {
        Self::new()
            .add_stage("preprocessing", B256::repeat_byte(0x01), 10_000_000, 64)
            .add_stage("feature_extraction", B256::repeat_byte(0x02), 30_000_000, 128)
            .add_stage("inference", B256::repeat_byte(0x03), 50_000_000, 256)
            .add_stage("postprocessing", B256::repeat_byte(0x04), 5_000_000, 32)
    }

    pub fn total_cycles(&self) -> u64 {
        self.stages.iter().map(|s| s.estimated_cycles).sum()
    }

    pub fn max_memory(&self) -> usize {
        self.stages.iter().map(|s| s.memory_mb).max().unwrap_or(0)
    }
}

impl Default for StagedPipeline {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Model Optimizer
// ═══════════════════════════════════════════════════════════════════════════════

/// Estimates whether a model fits in zkVM and provides recommendations.
pub struct ModelOptimizer;

impl ModelOptimizer {
    /// Recommendations for fitting large models in zkVM.
    pub fn recommendations() -> Vec<&'static str> {
        vec![
            "Use INT8 quantization (4x smaller than FP32)",
            "Apply pruning to remove unnecessary weights",
            "Use knowledge distillation for smaller models",
            "Split model into encoder/decoder stages",
            "Use lookup tables instead of complex operations",
            "Pre-compute expensive operations off-chain",
            "Use batch normalization folding",
            "Apply weight sharing where possible",
        ]
    }

    /// Estimate if a model will fit in zkVM.
    pub fn estimate_fit(
        parameters: usize,
        bits_per_param: usize,
        memory_limit_mb: usize,
    ) -> ModelFitEstimate {
        let model_size_mb = (parameters * bits_per_param) / 8 / 1024 / 1024;
        let fits = model_size_mb < memory_limit_mb;

        ModelFitEstimate {
            model_size_mb,
            memory_limit_mb,
            fits,
            recommendation: if fits {
                "Model should fit in zkVM".to_string()
            } else {
                format!(
                    "Model too large. Try quantization or split into {} stages",
                    (model_size_mb / memory_limit_mb) + 1
                )
            },
        }
    }
}

/// Estimate of whether a model fits.
#[derive(Debug)]
pub struct ModelFitEstimate {
    pub model_size_mb: usize,
    pub memory_limit_mb: usize,
    pub fits: bool,
    pub recommendation: String,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_segment_estimation() {
        let executor = ContinuationExecutor::new(LargeProgramConfig::default());

        assert_eq!(executor.estimate_segments(10_000_000), 1);
        assert_eq!(executor.estimate_segments(100_000_000), 2);
        assert_eq!(executor.estimate_segments(200_000_000), 4);
    }

    #[test]
    fn test_staged_pipeline() {
        let pipeline = StagedPipeline::ml_detection_pipeline();

        assert_eq!(pipeline.stages.len(), 4);
        assert_eq!(pipeline.total_cycles(), 95_000_000);
        assert_eq!(pipeline.max_memory(), 256);
    }

    #[test]
    fn test_model_fit() {
        // 1M parameters, INT8
        let small = ModelOptimizer::estimate_fit(1_000_000, 8, 256);
        assert!(small.fits);

        // 100M parameters, FP32
        let large = ModelOptimizer::estimate_fit(100_000_000, 32, 256);
        assert!(!large.fits);
    }

    #[test]
    fn test_config_defaults() {
        let config = LargeProgramConfig::default();
        assert_eq!(config.max_cycles_per_segment, 50_000_000);
        assert_eq!(config.max_segments, 2000);
        assert!(config.enable_compression);
    }

    #[test]
    fn test_bonsai_recommendation() {
        let config = LargeProgramConfig {
            bonsai_threshold_cycles: 50_000_000,
            ..Default::default()
        };
        let executor = ContinuationExecutor::new(config);
        // Can't test analyze_program without real ELF, but can test estimation
        assert_eq!(executor.estimate_segments(100_000_000), 2);
    }
}
