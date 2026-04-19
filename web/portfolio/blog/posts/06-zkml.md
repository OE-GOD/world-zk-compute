# How I Built a Zero-Knowledge Proof System for AI Inference (Solo, From Scratch)

A bank runs an XGBoost model to decide your credit application. The model says "denied." You ask: "Prove the model actually ran. Prove it ran on MY data. Prove you didn't change the answer."

The bank can't. There's no way to verify that an ML model produced a specific output without re-running the model yourself — which requires the bank's proprietary model, their infrastructure, and their data pipeline.

I built the system that solves this. It's called MOOUSER. It generates cryptographic proofs that an ML model produced a specific output on specific inputs. The proof is 629ms to verify off-chain, or 3-6M gas on-chain. The model's weights stay private. The proof is unforgeable.

I built it solo. 3,335 tests. 108 tasks shipped. Rust, Solidity, Go, TypeScript, Python. Open source under Apache-2.0.

Here's how it works, and what I learned building it.

---

## The Problem: AI Is a Black Box

When a company says "our AI model decided X," you have two options:

1. **Trust them.** They say the model ran, it produced this output, they didn't tamper with it. You have no way to check.
2. **Re-run it yourself.** You need the model weights, the inference code, the exact input preprocessing, and enough compute to run it. For most ML models, this is impossible — the weights are proprietary.

Zero-knowledge proofs offer a third option:

3. **Verify a proof.** The company generates a cryptographic proof alongside the inference. The proof mathematically guarantees that a specific model (identified by a hash) produced a specific output on specific inputs. You verify the proof in milliseconds. You never see the model weights.

## Why This Is Hard

A zero-knowledge proof says: "I know a witness W such that F(W) = output, and I can prove this without revealing W."

For ML inference, W is the model weights and F is the inference computation. An XGBoost model with 100 trees, depth 6, and 50 features performs ~6,400 comparisons, ~100 additions, and ~100 table lookups per inference. That's a small computation — but converting it to a ZK circuit is where the difficulty lies.

The standard approach (zkVM) puts the entire computation inside a virtual machine, generates a proof of the VM's execution trace, and converts that to a succinct proof. This works but is extremely expensive — a single XGBoost inference costs $5+ in proving time.

I used a different approach.

## The Architecture: GKR + Hyrax + Groth16

Instead of proving "this VM executed correctly" (general but slow), I prove "this specific arithmetic circuit computed correctly" (specialized but fast).

### Layer 1: GKR Protocol

The GKR (Goldwasser-Kalai-Rothblum) protocol proves that a layered arithmetic circuit was computed correctly. It works top-down: start with the claimed output, then verify each layer's computation by reducing it to a claim about the layer below.

For an XGBoost model with 100 trees at depth 6, the circuit has 88 compute layers. Each layer contains additions and multiplications over a finite field (BN254's scalar field, ~254-bit primes).

The key property: GKR verification is logarithmic in the circuit size. An 88-layer circuit with millions of gates requires only ~88 rounds of interaction (made non-interactive via Fiat-Shamir with Poseidon hashing).

### Layer 2: Hyrax Polynomial Commitments

GKR reduces the output claim to a claim about the inputs. To verify the input claim, the verifier needs to check that the committed inputs (model weights) match the claimed values at specific evaluation points.

Hyrax uses Pedersen commitments with inner-product arguments. The prover commits to the model weights once (when registering the model). For each inference, the prover shows that the committed weights, evaluated at the GKR's challenge point, give the expected value.

### Layer 3: Groth16 for EC Operations

The on-chain verifier needs to check elliptic curve operations (Pedersen commitments, multi-scalar multiplications). These are expensive in Solidity — ~200M gas for an 88-layer XGBoost circuit.

I built a hybrid approach: the Solidity verifier replays the Poseidon transcript (cheap, ~2M gas) and delegates the EC operations to a Groth16 proof (which verifies EC equations in ~230K gas via the EVM's BN254 precompiles).

The result: full verification in 3-6M gas instead of 200M+.

### The Full Stack

```
XGBoost model → Circuit compiler → GKR prover → Hyrax commitments
                                                        ↓
                              Groth16 wrapper (EC operations only)
                                                        ↓
                              On-chain verification (3-6M gas)
                              Or off-chain verification (629ms)
```

## What I Actually Built

This isn't a research prototype. It's a production system:

**Rust prover** (`prover/`): Generates GKR proofs, Hyrax commitments, and coordinates with the Groth16 wrapper. Handles BN254 field arithmetic, Poseidon hashing, and the full Fiat-Shamir transcript.

**Circuit compiler** (`examples/xgboost-remainder/`): Converts XGBoost JSON models into layered arithmetic circuits. Handles tree traversal (leaf selection via path bits), feature comparison (bit decomposition with sign extraction), and multi-tree aggregation.

**Solidity verifiers** (`contracts/src/remainder/`): On-chain verification contracts — PoseidonSponge, SumcheckVerifier, HyraxVerifier, GKRVerifier, RemainderVerifier, GKRHybridVerifier, GKRDAGVerifier, DAGBatchVerifier. 166 Foundry tests.

**DAG batch verifier**: The 88-layer XGBoost circuit exceeds Ethereum's 30M gas block limit. I built a multi-transaction verifier that splits verification into 15 transactions (1 setup + 11 compute batches + 3 finalize), each under 30M gas. Sessions are authenticated by sender address and stored on-chain.

**Stylus WASM verifier** (`contracts/stylus/`): Ported the entire GKR verifier to Rust/WASM for Arbitrum Stylus. The WASM binary is 52KB raw, 24.5KB Brotli-compressed — under the 24KB Stylus deployment limit. Key optimization: shared `#[inline(never)]` field operations that deduplicate Fq/Fr arithmetic, and a `verify!` macro that strips panic strings from the WASM build.

**4 SDKs**: Python, TypeScript, Go, Rust client libraries for submitting proofs, registering models, and querying verification status.

**TEE happy path**: For production, most inferences run in a Trusted Execution Environment (AWS Nitro) at $0.0001/inference. ZK proofs are only generated when someone disputes a result. This makes cryptographic accountability economically viable — you only pay for expensive proofs when someone actually challenges you.

## The Hardest Engineering Problems

### Problem 1: Poseidon Sponge Consistency

The GKR protocol uses Fiat-Shamir to make it non-interactive. Both prover and verifier must produce identical challenge sequences from the same transcript. I use Poseidon hashing (width=3, rate=2, 8 full + 57 partial rounds with PSE/Scroll BN254 constants).

The problem: any discrepancy between the Rust prover's Poseidon implementation and the Solidity verifier's implementation causes the challenge sequences to diverge, and verification fails silently. Debugging this is a nightmare — you get a "proof invalid" error with no indication of WHERE the transcripts diverged.

I solved this by building a transcript trace system that logs every squeeze/absorb operation on both sides and comparing them step-by-step in test fixtures.

### Problem 2: MLE Bit-Ordering Convention

Multilinear extensions (MLEs) evaluate polynomials over Boolean hypercubes. The bit-ordering convention (MSB-first vs LSB-first) must match between the circuit compiler, the prover, and the verifier.

The Remainder library uses MSB-first: `point[0]` is the most significant bit of the data index. This means `evaluateMLEFromData` uses `(w >> (n-1-i)) & 1`, and the tensor initialization processes coordinates in reverse.

I got this wrong three times before getting it right. Each time, the proofs verified on small circuits but failed on the full 88-layer XGBoost circuit because the bit-ordering errors only manifest when the number of variables exceeds a threshold.

### Problem 3: Fitting in 24KB (Stylus WASM)

Arbitrum Stylus limits deployed WASM binaries to 24KB (Brotli-compressed). The full GKR verifier with BN254 arithmetic, Poseidon hashing, sumcheck verification, and proof decoding compiled to 52KB raw / 30KB Brotli. 6KB over the limit.

Optimizations that worked:
- `#[inline(never)]` on shared field operations (Fq add, mul, sub used by both Fq and Fr types)
- `verify!` macro that replaces `assert!` with conditional returns (strips panic format strings from WASM)
- `#[cfg(not(target_arch = "wasm32"))]` on all Debug trait implementations
- `U256::from_be_slice()` instead of `try_into().unwrap()` (avoids pulling in formatting code for the unwrap panic)

Final binary: 52KB raw / 24.5KB Brotli. Under the limit with 500 bytes to spare.

### Problem 4: 512-bit Modular Reduction

BN254 field multiplication produces 512-bit intermediate results that must be reduced modulo a 254-bit prime. My initial implementation used repeated subtraction — correct but O(2^256) worst case. It worked on small inputs and hung on real proofs.

I replaced it with Knuth's Algorithm D (long division for multi-precision integers), adapted for 256-bit divisors. This runs in constant time regardless of input magnitude.

## The Numbers

| Metric | Value |
|--------|-------|
| Off-chain verification | 629ms |
| On-chain (direct GKR) | ~252M gas (88-layer circuit) |
| On-chain (Groth16 hybrid) | 3-6M gas |
| On-chain (batch, 15 txs) | <30M gas each |
| Stylus WASM binary | 24.5KB (Brotli) |
| TEE happy path cost | $0.0001/inference |
| ZK fallback cost | $0.02 (Groth16) / $5 (direct GKR) |
| Test count | 3,335 (Rust + Solidity + Go) |
| Solidity tests | 166 (Foundry) |
| Supported model types | 5 (XGBoost, LightGBM, RF, LR, NN) |

## What I Learned

**1. The hard part of ZK isn't the math — it's the plumbing.** The GKR protocol is well-defined. Implementing it correctly across three languages (Rust prover, Solidity verifier, Go gnark wrapper) with identical Poseidon transcripts, matching bit-ordering conventions, and correct field arithmetic is where the months go.

**2. Production ZK needs economic design, not just cryptographic design.** A $5 proof per inference is too expensive for production. The TEE happy path + ZK dispute fallback architecture makes it viable by only generating proofs when challenged. This is an economics insight, not a cryptography insight.

**3. Size constraints force real optimization.** The 24KB Stylus limit forced me to understand exactly where every byte in the WASM binary comes from. Without that constraint, I would have shipped a 100KB binary and never learned about inline(never) deduplication or panic string elimination.

**4. Test everything against the largest circuit you'll deploy.** My bit-ordering bugs only appeared on 88-layer circuits. My reduction algorithm only hung on real-sized field elements. Small test cases pass with incorrect code because they don't exercise edge cases. Always test at production scale.

---

*Code: [github.com/OE-GOD/world-zk-compute](https://github.com/OE-GOD/world-zk-compute)*
