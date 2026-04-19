# Compliant TTT Is Worse Than No TTT (And What That Means for the Leaderboard)

The top entries on OpenAI's Parameter Golf leaderboard use Test-Time Training (TTT) — adapting the model on validation data before scoring. The improvement is dramatic: 1.1118 BPB without TTT vs 1.0787 with TTT. That's -0.033 BPB, worth more than any architecture change.

There's a problem: it violates the competition's rules.

## The Compliance Issue

The competition's Condition 3 states: "The score at position t is computed from p_t(x_t). Only after that score is fixed may state be updated using x_t."

Translation: you must score each token BEFORE the model trains on it.

The current TTT does the opposite:
1. Train on ALL validation tokens for 6 epochs
2. THEN score ALL validation tokens

Every token is scored AFTER the model has trained on it 6 times. The model has memorized the answers before being tested. PRs #1487 and #1488 were already closed for exactly this violation.

## My Fix: Score-First TTT

I implemented a compliant alternative. Split the validation data into two halves:

1. **Score first half** (model hasn't adapted yet — baseline quality)
2. **Train on first half** (model adapts to the patterns it just scored)
3. **Score second half** (model has adapted — should be better)

Each token is scored BEFORE the model trains on it. Condition 3 satisfied.

## The Result

| Method | BPB | Compliant? |
|--------|-----|-----------|
| No TTT | 1.1118 | Yes |
| **Score-first TTT** | **1.1152** | **Yes** |
| 6-epoch TTT | 1.0787 | No |

Score-first TTT is **0.003 BPB worse** than no TTT at all.

Not worse than the non-compliant version — worse than doing nothing.

## Why It Fails

The model's EMA (Exponential Moving Average) weights are carefully tuned by ~4800 steps of training with cosine warmdown. They sit at a smooth minimum in the loss landscape.

When TTT trains on the first half of validation data with lr=0.0005, it perturbs these weights. The perturbation is small but it pushes the model off its smooth minimum. The second half gets scored with a slightly-worse model.

The damage compounds during quantization. GPTQ converts FP16 weights to int6. The perturbed weights quantize worse than the original smooth weights (less regular distribution = higher quantization error). Final quantized BPB: 1.1179 for score-first vs 1.0884 for non-compliant.

## What This Means for the Leaderboard

If compliance is enforced (and two PRs have already been closed for it), every entry using multi-epoch TTT gets rescored. The current #1 at 1.081 BPB becomes ~1.11 BPB.

But here's the uncomfortable finding: **compliant TTT doesn't just give less improvement — it gives negative improvement.** The only way to benefit from TTT in this competition is to violate the scoring rules.

This means the 0.033 BPB gap between "with TTT" and "without TTT" on the leaderboard is entirely an artifact of non-compliance. It's not a modeling improvement. It's a measurement exploit.

## The Mathematical Intuition

Why does score-first TTT hurt? Think of it as a bias-variance tradeoff:

- **First half (50% of tokens):** Scored at baseline quality. No benefit from adaptation.
- **Second half (50% of tokens):** Scored after training on first half. But the training adds noise to the weights, and the adaptation is only to the first half's distribution — which may differ from the second half's distribution.

The average quality: (baseline + slightly_worse) / 2 = slightly_worse_than_baseline.

The non-compliant version avoids this by training on ALL data for 6 epochs before scoring ANY of it. Every token benefits from full adaptation. But this is exactly what Condition 3 prohibits.

## What I Learned

**Test-time training in this competition is fundamentally about memorization, not generalization.** The model doesn't learn "general patterns from the validation data." It memorizes the specific tokens it will be scored on. A compliant version that can only use past tokens to help future tokens doesn't help — because the model was already well-trained on the same distribution.

**Compliance matters.** Not because the rules are arbitrary, but because they distinguish between genuine prediction quality (the model can predict unseen text) and memorization (the model has seen the answers). The entire point of BPB as a metric is to measure prediction quality. Non-compliant TTT breaks this measurement.

---

*This is part of a series on [14 Experiments, 13 Failures](https://aungmaw.vercel.app/blog/parameter-golf.html) in OpenAI's Parameter Golf competition.*
