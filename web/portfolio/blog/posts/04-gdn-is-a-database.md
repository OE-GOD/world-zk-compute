# GDN Is Not a Sequence Model — It's a Differentiable Database

Everyone treats the Gated DeltaNet (GDN) as "another recurrent model." It's not. Once you understand its state update equation, you realize it's doing something fundamentally different from transformers, Mamba, or any other architecture in the competition.

GDN is a differentiable key-value database with INSERT, UPDATE, and DELETE operations.

## The State Update Equation

At every token, GDN's state matrix S (size d×d) updates:

```
S_t = α_t · S_{t-1} · (I - β_t · k_t · k_tᵀ) + β_t · v_t · k_tᵀ
```

Three operations in one equation:

**Step 1: DECAY.** `α_t · S_{t-1}`
Uniformly forget everything. α close to 1 = remember. α close to 0 = flush.

**Step 2: ERASE.** `- α_t · β_t · S_{t-1} · k_t · k_tᵀ`
Surgically remove the OLD value stored for key k_t. The term `S_{t-1} · k_t` retrieves what's currently stored for that key. Subtracting it erases that specific association.

**Step 3: WRITE.** `+ β_t · v_t · k_tᵀ`
Store the NEW value v_t for key k_t.

**Retrieval:** `o_t = S_t · q_t`
Query q_t retrieves a weighted combination of stored values.

This is literally one step of SGD on the associative memory loss L = ½||S·k - v||². The delta (error) is (S·k - v), and the update corrects it. That's why keys must be L2-normalized — the derivation requires ||k|| = 1.

## How This Differs From Everything Else

**Transformer:** "What is the attention-weighted combination of ALL past values?" Exact retrieval, O(n) memory, O(n²) compute. Can't overwrite — the KV cache only grows.

**Mamba/S4:** "What does my compressed state vector say?" Lossy compression, O(d) memory, O(d) compute. Can only decay uniformly — no targeted forgetting.

**GDN:** "What value did I store for this key?" Targeted retrieval, O(d²) memory, O(d²) compute. CAN overwrite old entries (delta rule). Transformers and Mamba CANNOT overwrite.

The delta rule is GDN's unique advantage. When the text says "The cat is big. No, the cat is small," the transformer stores BOTH "big" and "small" in its KV cache. GDN erases "cat→big" and writes "cat→small." The state reflects current truth, not full history.

## The State Capacity Problem

Per head: S ∈ R^{64 × 64} = 4,096 entries.
8 heads: 32,768 total entries.

Transformer KV cache at seq_len=2048, 4 KV heads: 1,048K entries.

**GDN stores 3% of the information a transformer does.** It compensates by being selective — the delta rule keeps only the most relevant associations. But this is a fundamental capacity limitation.

## My Experiment: Can We Fix the Capacity Problem?

If state capacity is the bottleneck, doubling it should help. I tested:

- **8 heads × 64 dim:** 32K state entries, 21 GB GPU memory
- **4 heads × 128 dim:** 65K state entries, 11.8 GB GPU memory

Same parameter count. Double the state capacity per head. The interference analysis says retrieval should be cleaner: with d=64, random keys overlap at |k_i^T k_j| ≈ 1/√64 = 0.125. With d=128, overlap drops to 1/√128 = 0.088. Each head can cleanly store ~16 items instead of ~8.

**Result: 0.011 BPB worse.** Despite 2x state capacity and 44% less memory.

Why? Fewer heads means fewer parallel attention patterns. The forward pass is ~15% slower. In a 10-minute competition, that's ~300 fewer training steps. The training deficit outweighs the capacity gain.

## The Real Bottleneck

State capacity is NOT the bottleneck at this scale. Training compute is. The model is massively undertrained relative to its capacity — it sees 630M tokens of 10B available (6%). More steps would help. More state wouldn't.

This is a general finding about recurrent models in time-limited competitions: the theoretical capacity of the state matters less than the practical throughput of the training loop. A faster model that trains more steps will beat a higher-capacity model that trains fewer steps.

## What I Learned

**Understanding the math changes what experiments you run.** Before reading the state update equation, I would have tried random architecture modifications. After understanding that GDN is an associative memory with interference-limited retrieval, I tested the specific bottleneck the math predicted (key dimension). The experiment was principled. It just happened to be wrong about what was actually limiting.

**Knowing WHY something fails is more valuable than knowing THAT it fails.** The head_dim=128 experiment didn't improve BPP. But it proved that state capacity isn't the bottleneck, training throughput is. That knowledge redirects all future optimization away from architecture changes and toward training efficiency.

---

*This is part of a series on [14 Experiments, 13 Failures](https://aungmaw.vercel.app/blog/parameter-golf.html) in OpenAI's Parameter Golf competition.*
