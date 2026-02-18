//! Sybil Detection Guest Program for World ID
//!
//! Analyzes World ID registration patterns to detect coordinated fake identity
//! creation — a core threat to the "one person = one World ID" guarantee.
//!
//! Runs inside the RISC Zero zkVM and produces a proof that:
//! 1. The detection algorithm was executed correctly on the input data
//! 2. Flagged registrations genuinely trigger detection heuristics
//! 3. Risk scores are mathematically correct and reproducible
//!
//! 5 detection heuristics (each contributes to a per-registration risk score):
//! - Temporal Clustering: burst of registrations at same orb within time window
//! - Geographic Impossibility: impossible travel speed between registrations
//! - Session Anomaly: suspiciously fast iris scan sessions
//! - Orb Rate Abuse: orb exceeding max registrations per hour
//! - Quality Score Anomaly: statistical outliers in iris quality scores

#![no_main]
#![no_std]

extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;
use risc0_zkvm::guest::env;

risc0_zkvm::guest::entry!(main);

// ═══════════════════════════════════════════════════════════════════════════════
// Input / Output types
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(serde::Deserialize)]
struct SybilDetectionInput {
    registrations: Vec<Registration>,
    config: SybilConfig,
}

#[derive(serde::Deserialize, Clone)]
struct Registration {
    /// Nullifier hash (unique per registration)
    id: [u8; 32],
    /// Orb identifier
    orb_id: [u8; 16],
    /// Unix timestamp in seconds
    timestamp: u64,
    /// Location grid cell (geohash encoded as u32)
    geo_hash: u32,
    /// Iris scan session duration in seconds
    session_duration_secs: u32,
    /// Iris quality score (0-1000)
    iris_score: u32,
    /// Time from scan to verification in milliseconds
    verification_latency_ms: u32,
}

#[derive(serde::Deserialize)]
struct SybilConfig {
    /// Time window for temporal clustering (seconds)
    time_window_secs: u64,
    /// Geohash distance threshold for "nearby" locations
    geo_proximity_threshold: u32,
    /// Minimum registrations to form a suspicious cluster
    min_cluster_size: u32,
    /// Maximum travel speed (km/h) — above this = impossible travel
    velocity_threshold_kmh: u32,
    /// Minimum expected iris scan session duration (seconds)
    min_session_duration_secs: u32,
    /// Maximum allowed registrations per orb per hour
    max_registrations_per_orb_per_hour: u32,
}

#[derive(serde::Serialize)]
struct SybilDetectionOutput {
    /// Total registrations analyzed
    total_registrations: u32,
    /// Number of flagged registrations
    flagged_count: u32,
    /// IDs of flagged registrations
    flagged_ids: Vec<[u8; 32]>,
    /// Risk scores (0-1000) for each flagged registration
    risk_scores: Vec<u32>,
    /// Number of suspicious clusters detected
    cluster_count: u32,
    /// Breakdown of flags by detection type
    flags_breakdown: FlagsBreakdown,
    /// SHA-256 hash of the input data
    input_hash: [u8; 32],
}

#[derive(serde::Serialize)]
struct FlagsBreakdown {
    temporal_cluster: u32,
    geographic_impossible: u32,
    session_anomaly: u32,
    orb_rate_abuse: u32,
    quality_anomaly: u32,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Main entry point
// ═══════════════════════════════════════════════════════════════════════════════

fn main() {
    let input: SybilDetectionInput = env::read();

    let input_hash = compute_input_hash(&input);

    let (flagged_ids, risk_scores, cluster_count, breakdown) = detect_sybils(&input);

    let output = SybilDetectionOutput {
        total_registrations: input.registrations.len() as u32,
        flagged_count: flagged_ids.len() as u32,
        flagged_ids,
        risk_scores,
        cluster_count,
        flags_breakdown: breakdown,
        input_hash,
    };

    env::commit(&output);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Detection engine
// ═══════════════════════════════════════════════════════════════════════════════

/// Per-registration score accumulator
struct RegScore {
    temporal: u32,
    geographic: u32,
    session: u32,
    orb_rate: u32,
    quality: u32,
}

impl RegScore {
    fn new() -> Self {
        Self { temporal: 0, geographic: 0, session: 0, orb_rate: 0, quality: 0 }
    }
    fn total(&self) -> u32 {
        self.temporal + self.geographic + self.session + self.orb_rate + self.quality
    }
}

fn detect_sybils(
    input: &SybilDetectionInput,
) -> (Vec<[u8; 32]>, Vec<u32>, u32, FlagsBreakdown) {
    let regs = &input.registrations;
    let cfg = &input.config;
    let n = regs.len();

    if n == 0 {
        return (
            Vec::new(),
            Vec::new(),
            0,
            FlagsBreakdown {
                temporal_cluster: 0,
                geographic_impossible: 0,
                session_anomaly: 0,
                orb_rate_abuse: 0,
                quality_anomaly: 0,
            },
        );
    }

    let mut scores: Vec<RegScore> = Vec::with_capacity(n);
    for _ in 0..n {
        scores.push(RegScore::new());
    }

    // ── Heuristic 1: Temporal Clustering (max 200 pts) ──────────────────────
    // For each registration, count how many other registrations at the SAME orb
    // fall within the time window. If >= min_cluster_size → suspicious.
    let mut cluster_count: u32 = 0;
    let mut cluster_counted = vec![false; n];

    for i in 0..n {
        let mut same_orb_nearby = 0u32;
        for j in 0..n {
            if i == j {
                continue;
            }
            if regs[i].orb_id == regs[j].orb_id {
                let dt = if regs[i].timestamp > regs[j].timestamp {
                    regs[i].timestamp - regs[j].timestamp
                } else {
                    regs[j].timestamp - regs[i].timestamp
                };
                if dt <= cfg.time_window_secs {
                    same_orb_nearby += 1;
                }
            }
        }
        if same_orb_nearby >= cfg.min_cluster_size {
            // Score scales with cluster density: 200 * min(cluster_size/min_cluster_size, 1)
            let pts = if same_orb_nearby >= cfg.min_cluster_size.saturating_mul(2) {
                200
            } else {
                100u32.saturating_add(
                    100u32.saturating_mul(same_orb_nearby - cfg.min_cluster_size) / cfg.min_cluster_size
                )
            };
            scores[i].temporal = pts;

            if !cluster_counted[i] {
                cluster_count += 1;
                // Mark all members of this cluster as counted
                cluster_counted[i] = true;
                for j in 0..n {
                    if i != j && regs[i].orb_id == regs[j].orb_id {
                        let dt = if regs[i].timestamp > regs[j].timestamp {
                            regs[i].timestamp - regs[j].timestamp
                        } else {
                            regs[j].timestamp - regs[i].timestamp
                        };
                        if dt <= cfg.time_window_secs {
                            cluster_counted[j] = true;
                        }
                    }
                }
            }
        }
    }

    // ── Heuristic 2: Geographic Impossibility (max 300 pts) ─────────────────
    // If two registrations by close timestamps have very different geo_hashes,
    // that implies impossible travel speed.
    for i in 0..n {
        for j in (i + 1)..n {
            let dt = if regs[i].timestamp > regs[j].timestamp {
                regs[i].timestamp - regs[j].timestamp
            } else {
                regs[j].timestamp - regs[i].timestamp
            };
            if dt == 0 || dt > 3600 {
                continue; // Skip same-second or > 1 hour apart
            }

            let geo_dist = if regs[i].geo_hash > regs[j].geo_hash {
                regs[i].geo_hash - regs[j].geo_hash
            } else {
                regs[j].geo_hash - regs[i].geo_hash
            };

            if geo_dist <= cfg.geo_proximity_threshold {
                continue; // Nearby locations — fine
            }

            // Approximate: geo_dist units ≈ km, speed = dist / (dt/3600)
            // speed_kmh = geo_dist * 3600 / dt
            let speed_kmh = (geo_dist as u64).saturating_mul(3600) / dt;

            if speed_kmh > cfg.velocity_threshold_kmh as u64 {
                scores[i].geographic = 300;
                scores[j].geographic = 300;
            }
        }
    }

    // ── Heuristic 3: Session Anomaly (max 250 pts) ──────────────────────────
    // Iris scans that complete too quickly are suspicious — real biometric
    // capture requires minimum time for positioning + capture + quality check.
    for i in 0..n {
        if regs[i].session_duration_secs < cfg.min_session_duration_secs {
            // The shorter the session, the more suspicious
            if regs[i].session_duration_secs <= cfg.min_session_duration_secs / 4 {
                scores[i].session = 250;
            } else if regs[i].session_duration_secs <= cfg.min_session_duration_secs / 2 {
                scores[i].session = 175;
            } else {
                scores[i].session = 100;
            }
        }
    }

    // ── Heuristic 4: Orb Rate Abuse (max 200 pts) ──────────────────────────
    // Count registrations per orb in any 1-hour sliding window. If any window
    // exceeds the configured max, flag all registrations in that window.
    for i in 0..n {
        let mut count_in_hour = 0u32;
        let window_start = regs[i].timestamp;
        let window_end = window_start + 3600;

        for j in 0..n {
            if regs[j].orb_id == regs[i].orb_id
                && regs[j].timestamp >= window_start
                && regs[j].timestamp < window_end
            {
                count_in_hour += 1;
            }
        }

        if count_in_hour > cfg.max_registrations_per_orb_per_hour {
            let excess = count_in_hour - cfg.max_registrations_per_orb_per_hour;
            let pts = (100 + excess * 20).min(200);
            if pts > scores[i].orb_rate {
                scores[i].orb_rate = pts;
            }
        }
    }

    // ── Heuristic 5: Quality Score Anomaly (max 250 pts) ────────────────────
    // Compute mean and standard deviation of iris_score. Flag outliers that are
    // significantly below the mean (low-quality scans in bulk = fake iris).
    let sum: u64 = regs.iter().map(|r| r.iris_score as u64).sum();
    let mean = (sum / n as u64) as u32;

    let variance_sum: u64 = regs.iter().map(|r| {
        let diff = if r.iris_score > mean {
            (r.iris_score - mean) as u64
        } else {
            (mean - r.iris_score) as u64
        };
        diff * diff
    }).sum();
    let std_dev = isqrt(variance_sum / n as u64) as u32;
    let std_dev = if std_dev == 0 { 1 } else { std_dev };

    for i in 0..n {
        if regs[i].iris_score < mean {
            let deviation = mean - regs[i].iris_score;
            if deviation > std_dev * 3 {
                scores[i].quality = 250;
            } else if deviation > std_dev * 2 {
                scores[i].quality = 150;
            }
        }
    }

    // ── Aggregate results ───────────────────────────────────────────────────
    let threshold = 500u32; // Registrations scoring >= 500/1000 are flagged

    let mut flagged_ids = Vec::new();
    let mut risk_scores_out = Vec::new();
    let mut breakdown = FlagsBreakdown {
        temporal_cluster: 0,
        geographic_impossible: 0,
        session_anomaly: 0,
        orb_rate_abuse: 0,
        quality_anomaly: 0,
    };

    for i in 0..n {
        let total = scores[i].total().min(1000);
        if total >= threshold {
            flagged_ids.push(regs[i].id);
            risk_scores_out.push(total);
            if scores[i].temporal > 0 { breakdown.temporal_cluster += 1; }
            if scores[i].geographic > 0 { breakdown.geographic_impossible += 1; }
            if scores[i].session > 0 { breakdown.session_anomaly += 1; }
            if scores[i].orb_rate > 0 { breakdown.orb_rate_abuse += 1; }
            if scores[i].quality > 0 { breakdown.quality_anomaly += 1; }
        }
    }

    (flagged_ids, risk_scores_out, cluster_count, breakdown)
}

/// Integer square root (Babylonian method)
fn isqrt(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

// ═══════════════════════════════════════════════════════════════════════════════
// Input hashing
// ═══════════════════════════════════════════════════════════════════════════════

fn compute_input_hash(input: &SybilDetectionInput) -> [u8; 32] {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();

    for reg in &input.registrations {
        hasher.update(&reg.id);
        hasher.update(&reg.orb_id);
        hasher.update(&reg.timestamp.to_le_bytes());
        hasher.update(&reg.geo_hash.to_le_bytes());
        hasher.update(&reg.session_duration_secs.to_le_bytes());
        hasher.update(&reg.iris_score.to_le_bytes());
        hasher.update(&reg.verification_latency_ms.to_le_bytes());
    }

    hasher.update(&input.config.time_window_secs.to_le_bytes());
    hasher.update(&input.config.geo_proximity_threshold.to_le_bytes());
    hasher.update(&input.config.min_cluster_size.to_le_bytes());
    hasher.update(&input.config.velocity_threshold_kmh.to_le_bytes());
    hasher.update(&input.config.min_session_duration_secs.to_le_bytes());
    hasher.update(&input.config.max_registrations_per_orb_per_hour.to_le_bytes());

    let result = hasher.finalize();
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&result);
    hash
}
