# Security Model: World ZK Compute

**Last updated:** 2026-03-26
**Version:** 1.0
**Scope:** TEEMLVerifier, RemainderVerifier, TEE enclave, operator service, zkml-verifier crate
**Audience:** Security auditors, compliance reviewers, protocol engineers

---

## 1. System Overview

World ZK Compute provides verifiable ML inference for on-chain applications. An ML model (currently XGBoost tree ensembles) runs inside a Trusted Execution Environment (AWS Nitro enclave). The enclave signs each inference result with ECDSA. The signed result is submitted on-chain via the `TEEMLVerifier` contract, which accepts it after a challenge window. If any party disputes the result, the dispute is resolved by a cryptographic proof (GKR+Hyrax zero-knowledge proof verified on-chain via `RemainderVerifier`). This two-tier design provides low-cost happy-path verification (~3K gas) with a mathematically sound fallback (~3-6M gas hybrid, ~254M gas direct) that does not depend on hardware trust.

---

## 2. Trust Model

### 2.1 TEE Happy Path

The happy path trusts:

- **AWS Nitro hardware isolation.** The enclave runs in a hardware-isolated VM with no persistent storage, no interactive access, and a measured boot process. The enclave image hash (PCR0) is recorded on-chain at registration time.
- **ECDSA signature integrity.** The enclave generates a secp256k1 signing key internally. The attestation signature binds `(modelHash, inputHash, resultHash)` to the enclave's key via EIP-712 structured data signing.
- **Admin honesty for enclave registration.** The contract owner registers enclave keys and their image hashes. A compromised or malicious admin could register a rogue enclave. (See Section 6, Known Limitations.)

What the happy path does NOT trust:

- The submitter (any address can submit; stake is required).
- The network (EIP-712 domain separator includes `chainId` and `verifyingContract` for replay protection).
- External observers (the challenge window allows anyone to dispute).

### 2.2 ZK Dispute Path

The dispute path trusts only mathematics:

- **BN254 elliptic curve arithmetic.** All polynomial commitments and proof verification operate over the BN254 (alt-bn128) curve, which has a 254-bit scalar field and ~128-bit security.
- **Poseidon hash function.** The Fiat-Shamir transcript uses a Poseidon sponge (width=3, rate=2, 8 full + 57 partial rounds) over the BN254 scalar field. Collision resistance of Poseidon is required for non-interactive proof soundness.
- **GKR interactive proof protocol.** The Goldwasser-Kalai-Rothblum protocol reduces a claim about circuit output to a claim about circuit input via sumcheck reductions. Soundness relies on the Schwartz-Zippel lemma over the BN254 scalar field.
- **Hyrax polynomial commitment scheme.** Input layer claims are verified via Hyrax (Pedersen-based) polynomial commitments. Binding relies on the discrete logarithm assumption on BN254.
- **Groth16 SNARK (optional hybrid path).** When the hybrid Stylus+Groth16 path is used, the EC equation checks are wrapped in a Groth16 proof. This requires a trusted setup (circuit-specific CRS) and the knowledge-of-exponent assumption on BN254.

What the dispute path does NOT trust:

- The TEE hardware (proof is purely mathematical).
- The operator or submitter (proof verifies the actual computation).
- Any specific party (anyone can submit a dispute proof).

### 2.3 On-Chain Verifier

The on-chain verifier trusts:

- **Ethereum (or L2) consensus.** Block finality determines challenge window timing, result IDs, and payout execution.
- **EVM correctness.** The Solidity verifier contracts rely on EVM precompiles for EC operations (`ecAdd`, `ecMul`, `ecPairing` at addresses 0x06-0x08) and correct modular arithmetic.
- **Contract deployment integrity.** The deployed bytecode must match the audited source. The `RemainderVerifier` uses a UUPS proxy pattern; the admin controls upgrades.

---

## 3. Threat Model

### 3.1 Malicious Operator (Wrong Inference Result)

**Attack vector:** Operator submits an incorrect inference result with a valid TEE attestation (either by colluding with a compromised enclave or exploiting a TEE vulnerability).

**Defenses:**
- Submitter must post a stake (default 0.1 ETH) that is forfeited if the result is successfully challenged.
- Any address can challenge within the 1-hour challenge window by posting a bond (default 0.1 ETH).
- The ZK dispute path verifies the actual computation -- a wrong result cannot produce a valid GKR proof.
- If no ZK proof is submitted within the 24-hour dispute window, the challenger wins by timeout.

**Residual risk:** If no one challenges within 1 hour, the incorrect result finalizes. The system depends on at least one honest, attentive observer.

### 3.2 Compromised TEE (Forged Attestation)

**Attack vector:** TEE signing key is extracted via hardware side-channel attack, software exploit, or supply chain compromise. Attacker can forge attestations for arbitrary results.

**Defenses:**
- Admin can call `revokeEnclave(enclaveKey)` to block submissions from that key immediately.
- Already-submitted results can still be challenged; the ZK dispute path catches wrong results regardless of attestation validity.
- Enclave registration records the `enclaveImageHash` (PCR0) for forensic audit.
- The operator verifies Nitro attestation documents (COSE_Sign1, P-384 signature chain rooted at AWS CA) before registration.

**Response procedure:** Revoke enclave, challenge all pending results from that key, deploy new enclave with rotated key and fresh image.

### 3.3 Malicious Prover (Fake ZK Proof)

**Attack vector:** A party submits a fabricated GKR+Hyrax proof or Groth16 proof that claims to verify an incorrect computation.

**Defenses:**
- The GKR protocol has negligible soundness error (~2^{-128}) over the BN254 scalar field, derived from the Schwartz-Zippel lemma. A random field element matching a polynomial of bounded degree at a random evaluation point has probability at most `d/|F|` where `d` is the degree and `|F| ~ 2^254`.
- Hyrax commitments are computationally binding under the discrete log assumption on BN254.
- Groth16 proofs (when used) are knowledge-sound under the q-PKE assumption with circuit-specific trusted setup.
- The Fiat-Shamir transcript (Poseidon sponge) ensures challenges are deterministically derived from prior messages, preventing a prover from choosing favorable challenges.

**Residual risk:** A break in BN254 discrete log (e.g., quantum computer with sufficient qubits) would compromise both Hyrax commitments and Groth16 proofs. Current best classical attacks require ~2^128 operations.

### 3.4 Front-Running and MEV

**Attack vector:** MEV bots observe pending `challenge()` or `resolveDispute()` transactions in the mempool and attempt to extract value.

**Defenses:**
- `challenge()`: Front-runner must post the full bond (0.1 ETH) and assumes the same economic risk. Front-running a challenge is economically neutral to the protocol.
- `resolveDispute()` / `resolveDisputeByTimeout()`: Payouts go to `r.submitter` or `r.challenger` (addresses fixed at submission/challenge time). Front-running cannot redirect funds.
- `submitResult()`: `resultId` includes `msg.sender`, so a different submitter produces a different ID.
- `finalize()`: Permissionless; payout always goes to `r.submitter` regardless of caller.

See `docs/FRONT_RUNNING.md` for detailed per-function analysis.

### 3.5 Replay Attacks

**Attack vector:** Reuse a valid attestation from one chain on another, or replay an old attestation for a new submission.

**Defenses (implemented):**
- EIP-712 domain separator includes `chainId` and `verifyingContract` address, binding attestations to a specific chain and contract.
- The enclave includes a monotonic nonce and Unix timestamp in the signed payload.
- Nitro attestation freshness is validated with a configurable window (default 5-10 minutes).
- The operator computes a chain-bound nonce `keccak256(chainId || blockNumber || enclaveAddress)` for attestation requests.
- Enclave-side replay protection validates nonce uniqueness and timestamp freshness per request.

**Known gap:** The current `TEEMLVerifier.submitResult()` uses EIP-712 structured signing with `(modelHash, inputHash, resultHash)` as the typed data, which includes `chainId` via the domain separator. However, the enclave attestation module signs an extended payload that also includes `chainId`, `nonce`, and `timestamp` fields. The on-chain recovery currently verifies against the EIP-712 struct hash. If the on-chain verification is updated to include the extended fields, cross-chain replay would be fully prevented at the contract level.

### 3.6 Reentrancy

**Defenses:**
- All ETH-transferring functions (`resolveDispute`, `resolveDisputeByTimeout`, `finalize`, `resolveDisputeHybrid`, `resolveDisputeHybridChunked`) use OpenZeppelin `ReentrancyGuard` (`nonReentrant` modifier).
- State updates (e.g., `r.finalized = true`, `disputeResolved[resultId] = true`) occur before ETH transfers (checks-effects-interactions pattern).

### 3.7 Admin Key Compromise

**Attack vector:** Contract owner key is stolen. Attacker can register rogue enclaves, swap the `RemainderVerifier` to one that always returns true, change stake amounts (capped at 100 ETH), or pause the contract.

**Defenses:**
- `Ownable2Step` requires the new owner to explicitly accept, preventing accidental transfers.
- `Pausable` provides emergency stop but does not enable fund extraction.
- Stake and bond amounts are capped at 100 ETH per the setter functions.
- `RemainderVerifier` uses UUPS proxy with admin-only upgrade authorization.

**Recommendation:** Deploy with a multisig (e.g., Gnosis Safe) as owner. Add a timelock for `setRemainderVerifier()` since swapping the verifier is the highest-impact admin action.

---

## 4. Cryptographic Assumptions

| Assumption | Primitive | Security Level | Failure Consequence |
|---|---|---|---|
| **Discrete Log on BN254** | Hyrax commitments, Pedersen generators | ~128-bit classical | Prover could open commitments to false values |
| **Poseidon collision resistance** | Fiat-Shamir transcript (BN254 Fq) | ~128-bit (algebraic) | Prover could find transcript collisions, breaking non-interactivity |
| **Schwartz-Zippel lemma** | GKR sumcheck soundness | Negligible error (d/|F|) | False GKR proofs could verify |
| **Knowledge-of-exponent (q-PKE)** | Groth16 (trusted setup) | ~128-bit (pairing-based) | Fake Groth16 proofs could verify |
| **BN254 pairing non-degeneracy** | Groth16 verification, EC pairing checks | Standard assumption | Pairing-based proofs compromised |
| **SHA-256 preimage resistance** | Model hash, input hash, circuit hash | 128-bit | Hash collisions could alias different models/inputs |
| **secp256k1 ECDSA** | TEE attestation signatures | ~128-bit | Attestation forgery |

### Trusted Setup (Groth16 Only)

The Groth16 EC circuit wrapper uses a circuit-specific Common Reference String (CRS) generated by gnark. The toxic waste (tau) must be securely destroyed after setup. If the setup ceremony is compromised, a malicious party could forge proofs for the EC equation checks. This affects only the hybrid verification path, not the direct GKR+Hyrax path.

The direct GKR+Hyrax verification path requires NO trusted setup. All commitments are Pedersen-based with publicly derived generators.

---

## 5. Security Properties

### 5.1 Soundness

**Property:** An invalid proof (one that claims a computation produced output Y when it actually produced output Z) is rejected with overwhelming probability.

**Mechanism:**
- GKR sumcheck verification: Each layer reduction has soundness error at most `d/|F|` where `d` is the polynomial degree (2-3 for XGBoost circuits) and `|F| ~ 2^254`. With 88 layers, the total soundness error is bounded by `88 * 3 / 2^254 < 2^{-244}`.
- Hyrax commitment binding: Under the discrete log assumption, a committed value cannot be changed after commitment.
- Fiat-Shamir: Poseidon-based transcript ensures challenges are deterministic and unpredictable to the prover.

### 5.2 Completeness

**Property:** A valid proof (produced by an honest prover for a correct computation) is always accepted.

**Mechanism:** The verifier follows the same transcript protocol as the prover. If both sides implement the protocol correctly, all checks pass deterministically. The system has 166+ Solidity tests, 14 Go tests, and 34+ Rust tests validating the complete proof pipeline.

### 5.3 Zero-Knowledge

**Property scope:** The GKR+Hyrax proof system as used here provides *computational* zero-knowledge for committed inputs. The proof reveals the circuit structure (model topology) and public outputs, but does not reveal the private input features.

**What is revealed:**
- The circuit description (number of layers, layer types, connectivity) -- this is public and registered on-chain.
- The public outputs of the computation (model predictions).
- The Pedersen commitments to private inputs (computationally hiding under DL).

**What is NOT revealed:**
- The raw input feature values (protected by Pedersen commitment hiding).
- Intermediate computation values (GKR proofs operate on committed claims).

**Caveat:** If the model output itself leaks information about inputs (e.g., a binary classifier's score), that information is inherently revealed by the output, not by the proof system.

---

## 6. Known Limitations

### 6.1 Single Admin Key

The `TEEMLVerifier` uses `Ownable2Step` with a single owner address. The `RemainderVerifier` uses a UUPS proxy with a single admin. Until a multisig with timelock is deployed, a compromised admin key grants full control over enclave registration, verifier swapping, and contract pausing.

**Severity:** High
**Status:** Documented; multisig deployment recommended before mainnet.

### 6.2 No Formal Audit

The contract and prover code have not undergone a formal third-party security audit. The codebase includes extensive test coverage (166+ Solidity tests, 75+ Stylus WASM tests, 34+ Rust tests, 14 Go tests) and internal threat modeling, but this is not a substitute for independent review.

**Severity:** High
**Status:** Audit planned before production deployment.

### 6.3 Gas Costs Limit On-Chain Verification

Direct GKR+Hyrax verification costs ~254M gas, exceeding Ethereum L1's 30M block gas limit. On L1, the batch verifier (15 transactions) or hybrid Stylus+Groth16 path (~3-6M gas) must be used. On L2s like Arbitrum (with higher gas limits), direct verification is feasible in a single transaction.

**Implication:** On-chain dispute resolution is expensive. The economic security model requires that the prover stake (0.1 ETH) exceeds the ZK proof generation cost for disputes to be economically rational.

### 6.4 Challenge Window Assumptions

The 1-hour challenge window assumes at least one honest observer is monitoring the chain. If all observers are offline, colluding, or censored, an incorrect result finalizes unchecked. The protocol does not enforce correctness without an active challenger.

### 6.5 Enclave Nonce Reset

The monotonic nonce counter in the TEE enclave resets to 0 when the process restarts. On-chain, `resultId` uses `block.number` (not the enclave nonce), so this does not enable on-chain replay. However, it weakens off-chain attestation freshness guarantees during enclave restarts.

### 6.6 Groth16 Trusted Setup

The hybrid verification path uses Groth16, which requires a circuit-specific trusted setup. The current setup is performed by the development team using gnark's built-in setup. A multi-party ceremony (e.g., Powers of Tau) has not been conducted. A compromised setup would allow forging EC equation check proofs.

The direct GKR+Hyrax path and the Stylus WASM verifier path do NOT use Groth16 and are not affected by this limitation.

### 6.7 EIP-712 Attestation Field Mismatch

The enclave signs an extended payload including `chainId`, `nonce`, and `timestamp`, but the contract's `submitResult()` recovers the signer using only the EIP-712 struct hash of `(modelHash, inputHash, resultHash)` with the domain separator providing chain binding. The additional fields signed by the enclave are not verified on-chain. See Section 3.5 for details.

---

## 7. Contract Security Features

| Feature | Contract | Implementation |
|---|---|---|
| Access control | TEEMLVerifier | `Ownable2Step` (OpenZeppelin) |
| Access control | RemainderVerifier | UUPS proxy with `onlyAdmin` modifier |
| Reentrancy protection | Both | `ReentrancyGuard` (OpenZeppelin) on all ETH-transferring functions |
| Emergency pause | Both | `Pausable` (OpenZeppelin); `pause()` / `unpause()` by admin |
| Replay protection | TEEMLVerifier | EIP-712 domain separator with `chainId` + `verifyingContract` |
| Stake caps | TEEMLVerifier | `setChallengeBondAmount` / `setProverStake` capped at 100 ETH |
| Window bounds | TEEMLVerifier | Challenge window: 10min-7days; Dispute window: 1hr-30days |
| Upgradeability | RemainderVerifier | UUPS proxy (ERC-1967 storage slots); `_authorizeUpgrade` checks code exists |
| Circuit validation | RemainderVerifier | Registered circuits with description hash, generator hash, active flag |
| Batch session auth | DAGBatchVerifier | `msg.sender` check on every continue/finalize call |
| Custom errors | Both | Gas-efficient custom errors (no string reverts) |
| Storage packing | TEEMLVerifier | `PackedEnclaveInfo` (2 slots), `PackedMLResult` (~10 slots) |

---

## 8. Verification Paths Summary

| Path | Gas Cost | Trust Assumptions | Single Tx? | Production Status |
|---|---|---|---|---|
| TEE happy path | ~3K | TEE hardware + admin | Yes | Active |
| Direct GKR+Hyrax (Solidity) | ~254M | Math only | No (15 txs on L1) | E2E tested |
| Hybrid Stylus+Groth16 | ~3-6M | Math + Groth16 setup | Yes | E2E tested |
| DAG Batch (multi-tx) | ~17-28M/tx | Math only | No (15 txs) | E2E tested |
| Stylus WASM (direct) | ~5-25M | Math only | Yes (Arbitrum) | E2E tested |
