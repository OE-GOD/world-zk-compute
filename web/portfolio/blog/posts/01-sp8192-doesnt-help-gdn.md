# Why SP8192 Doesn't Help GDN-Hybrid (And What That Teaches About Architecture-Specific Optimization)

Every top entry on OpenAI's Parameter Golf leaderboard uses the SP8192 tokenizer. It produces 37% fewer tokens than SP1024. Fewer predictions, lower BPB. Simple math.

So I switched the GDN-Hybrid architecture from SP1024 to SP8192. It should have been a free -0.03 BPB improvement.

It wasn't. The artifact went from 14.59 MB to 16.94 MB — over the 16 MB budget. The model literally doesn't fit.

## The Math That Killed It

SP1024 embedding: 1,024 × 512 = 524K parameters
SP8192 embedding: 8,192 × 512 = 4,194K parameters
Extra cost: 3,670K parameters = ~2.75 MB at int6

The GDN-Hybrid has ~33.8M total parameters. The SP8192 embedding adds 10.8% more parameters, but those parameters are in the embedding table — a lookup table, not compute. You're spending 2.75 MB of your 16 MB budget on a dictionary, not on the model's ability to think.

I tried shrinking the model to compensate (dim=512 → dim=496). The artifact shrank to 16.30 MB — still over budget. And the BPB got worse because the model had less compute capacity.

## Why It Works for Transformers But Not GDN

The top transformer entries are ~35.9M parameters and fit SP8192 in 16 MB. The GDN-Hybrid is ~33.8M parameters and can't. Why?

The answer is parameter efficiency under compression. Transformer weights at int6+Brotli compress to ~0.42 bytes/param. GDN weights compress to ~0.43 bytes/param. This tiny difference — 0.01 bytes/param — across 33M parameters is 330 KB. Enough to push the artifact over budget.

GDN weights compress slightly worse because the recurrent state matrices have different weight distributions than transformer attention matrices. The GDN's delta rule creates weight patterns that are harder for generic compressors to exploit.

## What I Learned

**Optimizations don't transfer between architectures.** SP8192 is a proven win for transformers. It's a proven loss for GDN-Hybrid. The difference comes from parameter budget allocation and compression characteristics — properties that are invisible if you only look at the architecture diagram.

Before applying any "known good" technique from one architecture to another, compute the full parameter budget including the technique's overhead. If the overhead exceeds the headroom, it doesn't matter how good the technique is.

The boring calculation (embedding params × bits / compression ratio vs remaining budget) would have saved me $20 in GPU time and 6 hours of debugging package incompatibilities on RunPod.

---

*This is part of a series on [14 Experiments, 13 Failures](https://aungmaw.vercel.app/blog/parameter-golf.html) in OpenAI's Parameter Golf competition.*
