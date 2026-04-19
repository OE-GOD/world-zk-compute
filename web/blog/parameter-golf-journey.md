# 14 Experiments, 13 Failures, and What I Actually Learned About ML Compression

*A community college student's honest journey through OpenAI's Parameter Golf competition.*

---

## The Competition

OpenAI's Parameter Golf: train the best language model that fits in 16 MB. Score by how well it compresses English text (bits per byte). Lower is better. Everyone gets 10 minutes on 8×H100 GPUs. Same budget, same data. The leaderboard #1 sits at 1.081 BPB.

I spent two weeks, ~$150 in GPU compute, and tested 14 ideas. 13 failed. Here's everything I found.

---

## The One That Worked: ANS Compression (PR #1510)

**The question:** Everyone uses generic compressors (LZMA, Brotli) to pack model weights into 16 MB. But these compressors don't know the weights are quantized neural network parameters with specific distributions. Are they wasting space?

**The test:** I measured the entropy of int6-quantized weights and compared it to what LZMA actually achieves.

```
Theoretical minimum:  4.37 bits/param
ANS (mine):          4.37 bits/param  (within 11 KB of optimal)
LZMA (standard):     5.28 bits/param  (0.91 bits/param wasted)

Savings: 1.89 MB (17.2%)
```

**The fix:** I built a rANS (range Asymmetric Numeral Systems) encoder using per-layer frequency tables matched to the actual weight distribution. Information-theoretically optimal compression for this signal.

**Result:** 17% lossless improvement. 1.89 MB saved. Merged as PR #1510 on openai/parameter-golf.

**Why it worked:** Everyone optimized the model. Nobody optimized the compression step. The boring part had the most opportunity.

---

## The 13 That Failed

### Experiment 1: SP8192 Tokenizer on GDN-Hybrid

**Hypothesis:** The SP8192 tokenizer produces 37% fewer tokens than SP1024. Fewer tokens = fewer predictions = lower BPB. Every transformer on the leaderboard uses SP8192. The GDN-Hybrid architecture uses SP1024.

**What I expected:** -0.03 BPB. Simple config change.

**What actually happened:**

| Config | Quantized BPB | Artifact | Fits? |
|--------|--------------|----------|-------|
| SP1024 (baseline) | ~1.10 | 14.59 MB | Yes |
| SP8192 dim=512 | 1.062 | 16.94 MB | **No** |
| SP8192 dim=496 | 1.088 | 16.30 MB | **No** |

The SP8192 embedding table (8192 × 512 = 4.2M params) costs 3.7M more parameters than SP1024 (1024 × 512 = 524K). At int6 + zstd, that's ~1.5 MB extra. The 14.59 MB artifact becomes 16.94 MB — over the 16 MB budget.

Shrinking the model (dim=496) to compensate loses more from reduced capacity than it gains from better tokenization.

**Lesson:** SP8192 helps transformers where embeddings are a small fraction of total params. For GDN-Hybrid at 35M params, the embedding cost is proportionally too large. The optimization that works for transformers doesn't transfer to a fundamentally different architecture.

---

### Experiment 2: GDN head_dim=128 (4 Heads Instead of 8)

**Hypothesis:** GDN's recurrent state is a d×d matrix per head. With head_dim=64, each head stores a 64×64 = 4096-entry associative memory. Doubling to head_dim=128 gives 16,384 entries — 4× more storage per head. With 4 heads instead of 8, total parameter count is identical but state capacity doubles.

**From first principles:** GDN retrieves values by querying S·q. With random keys, the signal-to-interference ratio degrades as more items are stored. At d=64, clean retrieval fails beyond ~8 stored items. At d=128, the limit doubles to ~16. More state capacity should mean better predictions.

**What I found:**

| Config | Quantized BPB | GPU Memory | Train Loss |
|--------|--------------|------------|------------|
| 8 heads × 64 dim | 1.094 | 21 GB | 2.30 |
| 4 heads × 128 dim | 1.105 | **11.8 GB** | **2.31** |

**The surprise:** 44% less GPU memory. The model trains at nearly identical loss. But BPB is 0.011 worse because 4 heads × 128 dim runs ~15% slower per step → fewer steps in 10 minutes → less training.

**Lesson:** State capacity is NOT the bottleneck for GDN at this scale. The bottleneck is training compute (steps per wallclock). The 44% memory reduction is interesting for other applications but doesn't help in a time-limited competition.

---

### Experiment 3: Warmdown Fix for GDN-Hybrid

**What I found:** PR #1545's GDN-Hybrid has three training systems — warmdown, SWA, and Late QAT — that are configured but never execute. The default ITERATIONS=9999 with WARMDOWN_ITERS=3000 means warmdown starts at step 6999, but training only reaches ~2100 steps in 590 seconds. All three systems silently fail.

**The fix:** ITERATIONS=2200, WARMDOWN_ITERS=400.

**What happened:** Warmdown activated. SWA collected 3 checkpoints. Late QAT triggered. BPB improved from 1.024 to 1.021 (measured with GDN's eval code).

**The catch:** GDN-Hybrid's byte-counting code has a bug (see Experiment 4). Both numbers are wrong. The relative improvement (~0.003) is real, but the absolute numbers are inflated by ~14%.

---

### Experiment 4: The Byte-Counting Bug

**Discovery:** PR #1545's `build_sentencepiece_luts` function double-counts the leading space byte for ~65% of tokens.

```python
# PR #1545 (GDN-Hybrid) — BUGGY:
base_bytes[i] = len(piece[1:].encode("utf-8")) + 1  # +1 baked in

# Then in eval:
tb += (has_leading_space_lut[tgt] & ~is_boundary_token_lut[prev])  # +1 again
```

For token "▁the": correct bytes = 4 (" the"), but code counts 4 + 1 = 5.

```python
# PR #1517 (Reference transformer) — CORRECT:
piece = piece[1:]  # strip ▁
base_bytes[i] = len(piece.encode("utf-8"))  # no +1, eval adds it
```

**Impact:** ~14% BPB inflation for all GDN-Hybrid submissions. Reported 1.024 BPB is actually ~1.17.

**How I found it:** Independently derived the same finding as a community reviewer. Verified by comparing per-token byte counts between the reference and GDN implementations.

**Lesson:** When your BPB looks too good, check the denominator. The metric is bits/bytes — if bytes are wrong, everything is wrong.

---

### Experiment 5: Score-First Compliant TTT

**The problem:** The top entries use Test-Time Training (TTT) that trains on validation data for 6 epochs, then scores that same data. This violates the competition's Condition 3: "score before update." PRs #1487 and #1488 were closed for exactly this violation. PR #1517 was flagged by community reviewers.

**My solution:** 2-chunk score-first TTT. Score the first half of val data (no adaptation). Train on the first half. Score the second half (with adaptation from first half). Each token is scored BEFORE the model trains on it.

**What happened:**

| Method | BPB |
|--------|-----|
| No TTT | 1.1118 |
| **Score-first TTT** | **1.1152 (worse!)** |
| Non-compliant 6-epoch | 1.0787 |

Score-first TTT is 0.003 BPB WORSE than no TTT at all. The single-pass training on the first half perturbs the carefully-tuned EMA weights, hurting predictions on the second half. And when those perturbed weights are quantized by GPTQ, the damage compounds: the quantized sliding-window BPB is 1.1179 vs 1.0884 for the non-compliant version.

**Lesson:** TTT gains in this competition come almost entirely from training on the exact tokens you're being scored on — multiple times. A compliant version that trains on past tokens and scores future tokens doesn't help because the adaptation is too coarse and the perturbation too harmful. The entire TTT improvement on the leaderboard may be an artifact of non-compliance.

---

### Experiment 6: 13-Block GDN Architecture (Extra Layer)

**Hypothesis:** The baseline GDN-Hybrid has 12 blocks (10 GDN + 2 SWA) and 1.41 MB of unused artifact space. Adding one more GDN layer (13 blocks total) uses that headroom for more model capacity.

**Result:** Artifact fits (15.74 MB). But BPB is 1.097 — no better than 12 blocks. The extra layer doesn't converge enough in the same wall-clock time. More parameters ≠ better model when training time is the bottleneck.

---

### Experiment 7: ANS on GDN Weights

**Hypothesis:** ANS gave 17% improvement on transformer weights over LZMA. Maybe it helps GDN weights too.

**Result:** ANS gives 19.63 MB vs zstd's 14.59 MB for GDN weights. ANS is WORSE. GDN weights have different distribution characteristics than transformer weights — zstd already captures their structure.

**Lesson:** A compressor matched to one weight distribution doesn't generalize to another architecture.

---

### Experiment 8: Per-Matrix Quantization

**Hypothesis:** Attention Q and K matrices are less sensitive than V and MLP matrices. Use int4 for Q/K and int6 for the rest.

**Result:** MSE increased by +51% (int5) and +271% (int4) for Q/K matrices. Dead on arrival.

---

### Experiment 9: Post-TTT GPTQ Calibration

**Hypothesis:** GPTQ calibrates the quantizer on training data, but TTT modifies the weights. Re-calibrating GPTQ after TTT should reduce quantization error.

**Result:** +0.000 improvement. GPTQ's per-channel scaling already adapts to the post-TTT weight distribution. Re-calibration finds the same solution.

---

### Experiment 10: Progressive Depth Recurrence

**Hypothesis:** GPUs are 95% idle with a 17M param model on H100. More recurrence iterations are computationally free.

**Result:** Recurrence trades steps for depth ~1:1. K=3→K=4 gives 33% more depth but 33% fewer steps. The total effective work is identical. No improvement.

---

### Experiment 11: Word-Start Loss Weighting

**Hypothesis:** Word-start tokens are 52% of total loss (6.71 bits avg vs 5.07 for continuations). Upweighting them forces the model to focus on the hardest predictions.

**Result:** Marginal on GDN. The recurrent state already provides document-level context that helps word-start predictions. The weighting adds redundant emphasis.

---

### Experiment 12: Casefold Tokenization (SP8192)

**Hypothesis:** Case-insensitive tokenization reduces vocabulary redundancy. "The" and "the" map to the same token, freeing vocabulary entries for other patterns.

**Result:** Trained the tokenizer. The retokenized data didn't improve over standard SP8192. Case information carries predictive signal — removing it loses more than the vocabulary savings gain.

---

### Experiment 13: Reduced Batch Size

**Hypothesis:** Critical batch size B_crit ≈ 57K tokens for 17M params. The default batch of 786K tokens is 13.8× over B_crit. Reducing to 196K gives 4× more optimizer steps.

**Result:** More steps but worse BPB. The larger batch provides more stable gradients. The B_crit analysis applies to models at convergence, but our model is still in the rapid-learning phase where gradient quality matters more than step count.

---

## The Pattern

Four experiments changed the architecture. Three changed the training pipeline. Three changed the compression. Three changed the evaluation. All failed except ANS compression.

**The model works.** It's a well-tuned 35M parameter transformer (PR #1517) or GDN-Hybrid (PR #1545). Dozens of researchers spent weeks optimizing it. There's no easy -0.03 BPB hiding in an architecture change.

**The pipeline has bugs.** The byte-counting error inflated GDN BPB by 14%. The TTT compliance issue affected multiple top entries. The warmdown timing mismatch disabled three training systems. These are where the real findings were — not in the model, but in the measurement and training infrastructure around it.

**Negative results have value.** SP8192 doesn't help GDN. Score-first TTT is worse than no TTT. Per-matrix quantization destroys model quality. These findings save everyone else from trying the same things.

---

## What I'd Tell My Past Self

1. **Check the eval code before optimizing.** I spent days improving GDN BPB that was measured wrong. 10 minutes of code review would have saved all of it.

2. **Run the baseline first.** Half my experiments used broken software stacks (wrong triton/torch/FLA versions). The first run should always be: can I reproduce the known result?

3. **The competition rewards compliance, not cleverness.** Multiple top entries were flagged or closed for TTT violations. A compliant 1.11 is worth more than a non-compliant 1.08.

4. **$4 experiments are cheap. $150 of undirected experiments is expensive.** Each run on 8×H100 costs $4-8. I should have spent more time reasoning and less time running.

5. **The boring parts have the most opportunity.** This is the lesson from both ANS (compression) and the warmdown fix (training schedule). It's also the lesson from finding the byte-counting bug. Nobody looks at the boring parts. That's why they're wrong.

---

## The Numbers

| Experiment | Expected | Actual | Status |
|-----------|----------|--------|--------|
| ANS compression | -17% size | -17% size | **Worked** |
| SP8192 on GDN | -0.03 BPB | Over 16 MB | Failed |
| head_dim=128 | -0.005 BPB | +0.011 BPB | Failed |
| Warmdown fix | -0.01 BPB | -0.003 BPB | Marginal |
| Score-first TTT | -0.008 BPB | +0.003 BPB | Failed |
| Extra GDN layer | -0.005 BPB | ~0 BPP | Failed |
| ANS on GDN | Smaller artifact | Bigger artifact | Failed |
| Per-matrix quant | Less degradation | +51-271% MSE | Failed |
| Post-TTT GPTQ | -0.005 BPB | +0.000 BPP | Failed |
| Depth recurrence | -0.01 BPB | ~0 BPP | Failed |
| Word-start weighting | -0.005 BPB | Marginal | Failed |
| Casefold tokenizer | -0.01 BPP | ~0 BPP | Failed |
| Reduced batch | -0.008 BPB | Worse | Failed |
| Byte-counting bug | N/A | Found 14% inflation | **Discovery** |

---

## What's Real on the Leaderboard

After this journey, here's what I believe:

- **Architecture is near-optimal.** Transformers with parallel residuals and depth recurrence are close to the limit for 16 MB / 10 minutes.
- **TTT compliance matters.** Multiple top entries use non-compliant TTT. If enforcement happens, the real leaderboard is ~1.10-1.11, not 1.08.
- **The quantization gap (0.01-0.02 BPB) is real but hard to close.** QAT with Muon optimizer failed on the leaderboard. The optimizer mismatch (Muon assumptions vs STE gradient bias) is a concrete, unsolved problem.
- **The entropy floor for FineWeb is ~0.8-1.0 BPB.** Anyone reporting below 0.7 has a bug.

---

*I'm Aung Maw, 22, from Myanmar. CS student at Skyline College, San Francisco. I build things — MOOUSER (verifiable AI with zero-knowledge proofs, 3,335 tests), ANS compression for neural networks (PR #1510), and a lot of failed experiments that taught me more than any success could.*

*Open to: research engineering roles, AI safety fellowships, and teams where rigorous thinking matters more than leaderboard positions.*

*GitHub: [github.com/OE-GOD](https://github.com/OE-GOD) | PR #1510: [openai/parameter-golf/pull/1510](https://github.com/openai/parameter-golf/pull/1510)*
