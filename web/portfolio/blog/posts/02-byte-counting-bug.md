# I Found a Bug That Inflated the Leaderboard by 14%

The GDN-Hybrid architecture claimed 1.024 BPB on OpenAI's Parameter Golf leaderboard. That would have been first place by a wide margin.

The real number is ~1.17 BPB. Not even close.

## How BPB Works

BPB (bits per byte) = total cross-entropy bits / total bytes of text.

The numerator depends on how well the model predicts. The denominator depends on how you count bytes. Get the denominator wrong, and the BPB looks better than it is.

## The Bug

SentencePiece represents word-start tokens with a `▁` prefix. The token `▁the` represents the text " the" — a space followed by "the". That's 4 bytes.

The GDN-Hybrid's `build_sentencepiece_luts` function counts bytes like this:

```python
if piece.startswith("▁"):
    has_space[i] = True
    base_bytes[i] = len(piece[1:].encode("utf-8")) + 1  # strip ▁, add 1 for space
```

Then the eval function adds the space byte again:

```python
tb = base_bytes_lut[tgt]
tb += (has_leading_space_lut[tgt] & ~is_boundary_token_lut[prev])  # +1 again
```

For `▁the`: base_bytes = 3 + 1 = 4, then eval adds another +1 = 5. But the actual text " the" is 4 bytes. Every word-start token is overcounted by 1 byte.

## The Impact

~65% of tokens in English text are word-start tokens (they have the `▁` prefix). Each overcounted by 1 byte. Average token length is ~4.5 bytes. So:

- Overcounted bytes: 65% × 1 byte = 0.65 extra bytes per token
- Actual bytes: ~4.5 per token
- Inflation: 0.65 / 4.5 = 14.4%

BPB = bits / bytes. If bytes are inflated by 14.4%, BPB is deflated by 14.4%.

Reported 1.024 × 1.144 = **1.171 BPB actual**.

## The Reference Implementation Is Correct

The competition's reference `train_gpt.py` does it right:

```python
if piece.startswith("▁"):
    has_leading_space_np[token_id] = True
    piece = piece[1:]  # strip ▁
base_bytes_np[token_id] = len(piece.encode("utf-8"))  # NO +1
```

No double-counting. The +1 happens only in the eval function. Total is correct.

## How I Found It

I was investigating why our GDN-Hybrid BPB numbers seemed "too good." The model was getting 1.024 BPB — better than the transformer leaderboard #1 at 1.081. That's suspicious for an architecture that's theoretically less capable.

I wrote a test:

```python
for i in range(20):
    piece = sp.id_to_piece(i)
    text = sp.decode([i])
    correct = len(text.encode('utf-8'))
    raw = len(piece.replace('▁', ' ').encode('utf-8'))
    if raw != correct:
        print(f'BUG: {piece} raw={raw} correct={correct}')
```

Every `▁` token showed `raw` 1 byte higher than `correct`. The bug was confirmed.

A community reviewer independently found the same issue and posted it on PR #1545. Multiple GDN-Hybrid PRs (#1545, #1553, #1576) all share this bug.

## What I Learned

**When a metric looks too good, check the denominator.** BPB has a numerator (model quality) and a denominator (byte counting). Everyone optimizes the numerator. Nobody checks the denominator.

This is the same pattern as my ANS compression finding: the "boring" part of the pipeline (byte counting, compression) is where the bugs hide. The model gets all the scrutiny. The infrastructure gets none.

**10 minutes of reading the eval code would have saved days of optimization.** I spent multiple days optimizing GDN-Hybrid BPB, debugging package incompatibilities, tuning warmdown schedules — all on numbers that were wrong by 14%. The entire time, the answer was in a 6-line function I never read.

---

*This is part of a series on [14 Experiments, 13 Failures](https://aungmaw.vercel.app/blog/parameter-golf.html) in OpenAI's Parameter Golf competition.*
