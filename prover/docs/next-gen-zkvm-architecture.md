# Next-Gen zkVM Architecture: Binius + Jolt + Groth16

**Status**: Design Document (Feb 2026)
**Scope**: How a composite Binius + Jolt + Groth16 proof system maps to the existing World ZK Compute prover architecture

---

## 1. Three-Layer Proof Architecture

The next-generation proving stack replaces the monolithic STARK-then-Groth16 pipeline with three specialized layers, each optimized for its role:

```
Layer 3: On-Chain Verification
  Groth16 SNARK (constant-size proof, ~230K gas)
       |
Layer 2: Instruction Dispatch
  Jolt (sumcheck + lookup arguments over multilinear polynomials)
       |
Layer 1: Field Arithmetic & Commitments
  Binius (binary field tower, FRI-Binius polynomial commitments)
```

### Layer 1: Binius (Binary Field Arithmetic)

Binius operates over binary tower fields (GF(2), GF(2^8), GF(2^16), ...) instead of the large prime fields used by risc0/Plonky2. This eliminates the impedance mismatch between CPU-native binary operations and field arithmetic:

- **Native binary operations**: AND, XOR, shifts map directly to binary field operations with zero overhead. A 32-bit addition that costs hundreds of prime-field constraints becomes a single binary-field operation.
- **FRI-Binius commitments**: A variant of FRI adapted for binary fields. Achieves O(n) prover time for polynomial commitments (vs O(n log n) for standard FRI over prime fields).
- **Small field elements**: GF(2^8) elements are 1 byte vs 32 bytes for a 256-bit prime field element. This 32x reduction in element size translates to proportional memory savings in the execution trace.

### Layer 2: Jolt (Instruction Dispatch via Lookups)

Jolt proves RISC-V instruction execution using lookup arguments rather than custom circuits:

- **Lasso lookups**: Each RISC-V instruction (ADD, MUL, LOAD, etc.) is decomposed into lookups against precomputed tables. The prover demonstrates that each instruction's output matches the table entry for its inputs.
- **Sumcheck protocol**: Instead of committing to a monolithic constraint polynomial and using FRI to verify, Jolt uses the sumcheck protocol to verify multilinear polynomial evaluations interactively. Sumcheck is O(n) for the prover.
- **No per-instruction circuits**: Unlike risc0 (which has custom STARK constraints for each instruction type), Jolt treats all instructions uniformly via lookups. This simplifies the proof system and reduces the trusted codebase.

### Layer 3: Groth16 (On-Chain Verification)

The Jolt+Binius proof is wrapped in a Groth16 SNARK for on-chain verification:

- **Constant-size proof**: ~192 bytes regardless of computation size
- **Constant-time verification**: ~230K gas on Ethereum/World Chain
- **Recursive composition**: The Groth16 circuit verifies the Jolt sumcheck transcript + Binius FRI-Binius decommitment

This is the same final step used today with risc0 (STARK -> Groth16). The Groth16 wrapper circuit changes (it verifies sumcheck instead of FRI), but the on-chain verifier interface remains the same.

---

## 2. M3 Arithmetization and Decomposition

### What is M3?

M3 (Multi-Multiset Matching) is Binius's arithmetization scheme. It replaces the single monolithic execution trace used by STARKs with multiple per-operation-type tables connected by channels:

```
Traditional STARK (risc0 v3.0):
  One giant trace table: [PC | opcode | rs1 | rs2 | rd | mem_addr | mem_val | ...]
  All constraints checked against this single table

M3 (Binius):
  ALU table:     [opcode | in_a | in_b | out]
  Memory table:  [addr | value | timestamp]
  Range table:   [value | bound]
  ...connected by multiset matching channels
```

Each table contains only the columns relevant to its operation type. Channels enforce that data flowing between tables is consistent (e.g., a memory write in the ALU table matches a row in the memory table).

### Mapping to Existing Code

The M3 decomposition pattern maps directly to the existing `InputDecomposer` and strategy patterns in the codebase:

| M3 Concept | Existing Code | Role |
|-----------|--------------|------|
| Operation tables | `DecompositionStrategy` | Each strategy produces a typed sub-trace |
| Channels | `InputDecomposer` routing | Connects inputs to correct strategy |
| Table commitments | `ZkVmBackend::prove()` | Backend commits to each table |
| Multiset checks | Verification in `prove_with_snark()` | Cross-table consistency |

The key insight is that our `InputDecomposer` already separates computation by type — when M3 becomes available, each decomposition strategy can map to an M3 table type without changing the external interface.

---

## 3. Sumcheck Replaces FFTs

### Current Pipeline (risc0 v3.0)

```
Guest execution
  -> Execution trace (one row per cycle, ~200 columns)
  -> Segment into chunks (2^po2 rows each)
  -> Per-segment STARK:
       Commit to trace polynomials (NTT / FFT over prime field)
       Compute constraint polynomial
       FRI proof (recursive halvings with FFTs)
  -> Groth16 wrapper (verify all segment STARK proofs)
```

**Bottleneck**: FFT/NTT operations are O(n log n) per segment. For a 100M-cycle program with po2=20, that's ~100 segments each requiring FFTs over 2^20-element polynomials.

### New Pipeline (Jolt + Binius)

```
Guest execution
  -> Execution trace (multilinear polynomial representation)
  -> Jolt instruction proofs:
       Per-instruction lookup argument (Lasso)
       Sumcheck over multilinear polynomials: O(n)
  -> Binius commitment:
       FRI-Binius polynomial commitment: O(n)
       No NTT/FFT required (binary field packing instead)
  -> Groth16 wrapper (verify sumcheck transcript)
```

**Improvement**: The entire prover is O(n) in the trace size. No FFTs. The constant factors are also smaller because binary field operations are cheaper than prime field operations.

### Performance Impact

| Operation | risc0 v3.0 (prime field) | Jolt+Binius (binary field) |
|-----------|------------------------|---------------------------|
| Trace commitment | O(n log n) via NTT | O(n) via binary packing |
| Constraint evaluation | O(n) | O(n) via sumcheck |
| Polynomial opening | O(n log n) via FRI | O(n) via FRI-Binius |
| **Total prover** | **O(n log n)** | **O(n)** |

---

## 4. Integration Points

The existing prover architecture is already designed for multi-backend proving. Here's how each component maps to the future Jolt+Binius backend:

### `ZkVmBackend` Trait (unchanged)

```rust
#[async_trait]
pub trait ZkVmBackend: Send + Sync {
    async fn execute(&self, elf: &[u8], input: &[u8]) -> Result<ExecutionResult>;
    async fn prove(&self, elf: &[u8], input: &[u8]) -> Result<ProofResult>;
    async fn prove_with_snark(&self, elf: &[u8], input: &[u8]) -> Result<ProofResult>;
}
```

This trait is backend-agnostic by design. A Jolt+Binius backend implements `prove()` with sumcheck-based proving and `prove_with_snark()` with a Groth16 wrapper — the caller never needs to know which proof system is used.

### `MultiVmProver` Router (unchanged)

The router dispatches based on `ZkVmType` detection. Adding Jolt is the same pattern as adding SP1:

```rust
// Already implemented in this PR:
backends.insert(ZkVmType::Jolt, Box::new(JoltBackend::new()?));
```

### `FastProver` Strategy Selection (unchanged)

Jolt manages its own trace segmentation internally (similar to SP1), so the `FastProver` correctly routes Jolt programs to `ProvingStrategy::Direct`. No segmented or continuation strategies are needed.

### `InputDecomposer` (unchanged)

Input decomposition is orthogonal to the proving backend. The same decomposed inputs feed into whichever `ZkVmBackend` handles the program. When M3 tables become available, the decomposer's output can optionally be structured to match M3 table schemas, but this is not required — the guest program handles its own internal data layout.

### On-Chain Verifier Contracts

The on-chain verifier interface remains the same: `verify(seal, imageId, journalDigest)`. What changes is:
- The `seal` format (Groth16 proof bytes wrapping a sumcheck transcript instead of a STARK)
- The verifier contract implementation (different Groth16 verification key)
- The `imageId` computation (different commitment scheme)

The `ExecutionEngine` contract's interface is unaffected — it just passes the seal to a verifier.

---

## 5. Migration Path

### Phase 1: Jolt Backend Skeleton (This PR, Q1 2026)

- `JoltBackend` struct implementing `ZkVmBackend` with placeholder errors
- Full wiring into `MultiVmProver`, `FastProver`, detection logic
- Feature-gated behind `--features jolt`
- No external dependency (self-contained skeleton)

**Deliverable**: Architecture is in place. Jolt programs are detected and routed correctly. Clear error messages explain what's pending.

### Phase 2: Jolt Stabilization + On-Chain Verifier (~Q3 2026)

- jolt-sdk stabilizes programmatic ELF loading API
- Fill in `execute()` and `prove()` with real Jolt proving
- Deploy Jolt verifier contract on testnet
- Benchmark Jolt proving times vs risc0

**Dependencies**: jolt-sdk API stabilization, Jolt verifier contract development

### Phase 3: Binius Commitment Integration (~Q4 2026)

- Jolt switches from Hyrax/KZG commitments to Binius binary field commitments
- Prover memory usage drops significantly (binary field elements are smaller)
- FRI-Binius replaces standard FRI for polynomial openings
- Benchmark shows O(n) prover time improvement

**Dependencies**: Binius-Jolt integration in upstream jolt repository

### Phase 4: Production Composite System (2027+)

- Groth16 wrapper for Jolt+Binius proofs
- On-chain verifier deployed on World Chain mainnet
- Guest program portability: tooling to compile risc0 guests for Jolt
- Production benchmarks and optimization
- Gradual migration of existing programs from risc0 to Jolt

**Dependencies**: Groth16 wrapper circuit, audited verifier contracts, guest toolchain

---

## 6. Performance Projections

| Metric | risc0 v3.0 (current) | Jolt (alpha) | Jolt + Binius (projected) |
|--------|---------------------|--------------|--------------------------|
| **Prover time** (100M cycles) | ~6-7 min (Groth16) | ~2-3 min (est.) | ~30-60 sec (projected) |
| **Proof size** (compressed) | ~200 KB (STARK) | ~100-150 KB (est.) | ~50-80 KB (projected) |
| **Proof size** (SNARK) | ~256 bytes (Groth16) | N/A yet | ~256 bytes (Groth16) |
| **Verifier gas** | ~230K (Groth16) | N/A yet | ~230K (Groth16) |
| **Prover memory** | ~8-16 GB | ~4-8 GB (est.) | ~2-4 GB (projected) |
| **Prover complexity** | O(n log n) | O(n log n)* | O(n) |

*Jolt alpha uses Hyrax commitments (O(n log n)). Switching to Binius commitments achieves O(n).

**Important**: Jolt alpha numbers are estimates based on published benchmarks. Actual performance depends on program complexity, hardware, and API maturity. Binius projections are theoretical based on asymptotic improvements.

---

## 7. Risk Assessment

### API Instability (High Risk, Short-term)

Jolt's programmatic API for loading arbitrary ELF binaries is not stable. The `#[jolt::provable]` macro works for integrated builds, but our use case (loading pre-compiled ELF bytes at runtime) requires an API that may change significantly.

**Mitigation**: The skeleton backend returns clear errors. No production code depends on Jolt internals. The `jolt` feature flag ensures zero impact when disabled.

### Guest Portability (Medium Risk, Long-term)

Existing risc0 guest programs cannot run on Jolt without recompilation. Key differences:
- Different syscall interfaces (risc0 uses `env::read()`, Jolt uses `jolt_sdk::read()`)
- Different memory layouts and segment models
- Different precompile/accelerator support

**Mitigation**: Guest programs are already compiled to standard RISC-V ELFs. The core computation logic is portable. Only the I/O shim layer needs to change. A compatibility crate could abstract over both.

### On-Chain Verification Timeline (Medium Risk)

Jolt's Groth16 wrapper and on-chain verifier contract are not yet available. Without on-chain verification, Jolt proofs cannot be submitted to the ExecutionEngine contract.

**Mitigation**: The `prove_with_snark()` method returns a clear error. The system continues to use risc0 for production proving while Jolt matures. A testnet verifier deployment can happen before mainnet.

### Binius Maturity (Low Risk, Long-term)

Binius is even earlier than Jolt. The FRI-Binius construction works in theory and has reference implementations, but production-grade integration with Jolt is not yet available.

**Mitigation**: Binius is Phase 3/4. The architecture is designed so that Binius commitment integration happens inside Jolt's internals — our `ZkVmBackend` interface doesn't change regardless of which commitment scheme Jolt uses.

### Performance Regression (Low Risk)

Early Jolt versions may be slower than optimized risc0 v3.0 for some programs, especially those that benefit from risc0's custom precompiles (SHA-256, ECDSA, etc.).

**Mitigation**: The `MultiVmProver` router allows per-program backend selection. Programs can stay on risc0 until Jolt matches or exceeds its performance for their specific workload.
