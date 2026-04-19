# How I Beat OpenAI's Leaderboard With Better Compression

*A 22-year-old community college student's journey through OpenAI's Parameter Golf competition — from first principles to a PR on OpenAI's repo.*

---

## The Challenge

OpenAI's Parameter Golf: train the best language model that fits in 16 MB. Score by how well it compresses English text (bits per byte). Lower is better.

The catch: everyone gets the same 10 minutes on 8×H100 GPUs. Same budget. Same data. Whoever squeezes the most intelligence into 16 megabytes wins.

When I started, the leaderboard looked impossible. PhD researchers and ML engineers with years of experience were stacking technique after technique — depth recurrence, test-time training, custom tokenizers, exotic optimizers. I had none of that expertise.

So I asked a different question.

## The Wrong Question and the Right One

Everyone asked: **"How do I make a better model?"**

I asked: **"Where are the current models wasting space?"**

Same 16 MB budget. Same goal. Completely different approach. Instead of adding techniques, I looked for waste to remove.

## Experiment 1: Are Adjacent Layers Redundant?

**Hypothesis:** Transformer layers learn similar features. Store one layer fully, store the rest as small deltas.

**Test:** Measured the delta/weight ratio between adjacent layers.

**Result:** Delta/weight = 1.3. The deltas were BIGGER than the weights themselves. Layers are unique, not redundant.

**Verdict:** Dead end. But now I knew: the model isn't wasting parameters on redundant layers.

## Experiment 2: Are Embeddings Low-Rank?

**Hypothesis:** The embedding table is a flat lookup with massive redundancy. Factorize it with SVD.

**Test:** Computed SVD on the embedding matrix. Checked how many singular values capture 95% of variance.

**Result:** Embeddings are only 1.6% of the model and are high-rank. Even 50% savings = 0.8% of total budget.

**Verdict:** Dead end. Too small to matter.

## Experiment 3: Are Weights Spatially Correlated?

**Hypothesis:** Adjacent weights in a row are correlated (like pixels in an image). Use predictive coding.

**Test:** Compared direct entropy vs residual entropy (predict each weight from its neighbor).

**Result:** Residual entropy was 11.4% HIGHER than direct entropy. Adjacent weights are uncorrelated. Neural networks are not images.

**Verdict:** Dead end. Predictive coding makes things worse.

## Experiment 4: Is the Compressor Wasting Space?

**Hypothesis:** LZMA (the generic compressor everyone uses) doesn't know these are quantized neural network weights with a specific distribution.

**Test:** Measured the entropy of int6-quantized weights. Compared theoretical minimum vs LZMA output.

**Result:**

```
Theoretical minimum:  4.37 bits/param
ANS (mine):          4.37 bits/param  (within 11 KB of optimal!)
LZMA (standard):     5.28 bits/param  (0.91 bits/param wasted)

Savings: 1.89 MB (17.2%)
```

**Verdict:** CONFIRMED. LZMA wastes 17% of the compression budget. The fix: replace it with rANS (range Asymmetric Numeral Systems) using per-layer frequency tables.

## What 1.89 MB Buys

At int6 quantization, 1.89 MB = 2.58 million extra parameters. That's a 15% bigger model in the same 16 MB artifact — for free, with zero quality loss.

The compression is lossless. Same weights, smaller file. ANS encodes each int6 symbol in exactly `-log2(frequency/total)` bits. Information-theoretically optimal.

## The Result

I integrated ANS into the competition's training script and ran it on 8×H100:

```
Baseline (standard compression):  val_bpb = 1.2265, artifact = 15.83 MB
ANS + full stack:                  val_bpb = 1.0996, artifact = 13.56 MB
```

**1.0996 BPB — beating every merged entry on the leaderboard at the time of testing.**

The artifact was only 13.56 MB. Everyone else is crammed at 15.9 MB. I had 2.44 MB of unused budget — enough for an even bigger model.

## The Failed Experiments Mattered

Four experiments failed. One succeeded. But the failures weren't wasted — each one narrowed the search space:

| Experiment | Result | What I Learned |
|-----------|--------|---------------|
| Layer deltas | Failed | Layers carry unique information |
| Embedding SVD | Failed | Too small to matter |
| Spatial correlation | Failed | Weights aren't like pixels |
| **ANS compression** | **Worked** | **Generic compressors waste 17%** |
| Entropy regularization | Failed | Can't force compressibility during training |
| Match model | Marginal | Neural model already captures repetition |

The pattern: **the model itself is efficient. The waste is in the pipeline around it.** Nobody was optimizing the compression step because it's "the boring part." That's exactly why the opportunity existed.

## The Deeper Insight

After my experiments, I realized something the leaderboard wasn't seeing:

```
FP16 model (before quantization):   ~1.05 BPB
Best int6 model (after quantization): 1.07 BPB
Quantization damage:                  0.02 BPB
```

Architecture improvements gained 0.15 BPB over the competition. Training improvements gained 0.04 BPB. Then quantization gives back 0.02 BPB — 13% of all the gains from a month of work.

Everyone treats quantization as "the boring last step." But it's the single largest source of remaining fixable loss. The model is already good enough in FP16. The bottleneck is the quantization pipeline.

## What I'd Do With More Time

The competition runs until April 30. With more GPU time, I'd test:

1. **SDClip k-sweep** — the clipping threshold for quantization is set by convention (k=12.85), not optimization. My local tests show k=8 is optimal for this model.

2. **Post-TTT GPTQ calibration** — current entries calibrate the quantizer on training data, then adapt the model with test-time training. The quantizer is calibrated for weights that no longer exist. Fix: re-calibrate after adaptation.

3. **Progressive depth recurrence** — the GPUs are 95% idle (17M params on H100 is absurdly memory-bound). More recurrence iterations are computationally free. The training just needs gradient detaching and progressive scheduling to support K>2.

Each technique targets the quantization pipeline, not the model architecture. That's where the remaining waste is.

## The PR

[github.com/openai/parameter-golf/pull/1510](https://github.com/openai/parameter-golf/pull/1510)

The ANS compressor is open source. Anyone can integrate it into their submission with `USE_ANS=1`. It's a drop-in replacement that saves 17% on any model.

## What I Learned

**1. Measure before you optimize.** Four of my six experiments failed. Each failure took 30 minutes of testing and saved days of building the wrong thing.

**2. Optimize the right metric.** Everyone optimizes FP16 loss. The competition scores post-quantization BPB. These are different objectives with different optima.

**3. The boring parts have the most opportunity.** Architecture and training are where the talent concentrates. Compression and quantization are where the waste concentrates. Go where the waste is.

**4. First principles beat technique stacking.** I didn't add a single novel architecture component. I just asked "where are the bytes going?" and followed the answer.

---

*I'm Aung Maw, 22, from Myanmar, studying CS at Skyline College in San Francisco. I build things. [aungmaw.vercel.app](https://aungmaw.vercel.app)*

*Currently applying to: Thiel Fellowship, Mechanize, Anthropic AI Safety Fellows, and anything else where first-principles thinking matters more than credentials.*
