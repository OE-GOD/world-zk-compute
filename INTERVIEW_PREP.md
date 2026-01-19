# Interview Prep: Tools for Humanity - Security Engineering Internship

**Interview:** Friday, January 23, 2026, 1:00 PM PST (30 min)
**Type:** Preliminary Recruiter Screen with Kayla Kahl
**Role:** Security Engineering Internship - Detection & Response Team
**Zoom:** https://toolsforhumanity.zoom.us/j/98734493045

---

## 🎯 Key Insight: You Built EXACTLY What They Want!

From their job description:
> "We want to execute a smart contract that incentivizes any third party to run a specific set of code, against a specific set of data, publish the output, and prove that the calculation was done correctly. This is blockchain-based verifiable compute for Detection Engineering."

**This is literally World ZK Compute!** You have a massive advantage.

---

## 📚 Core Concepts (Understand to Explain!)

### What is Zero-Knowledge Proof? (ELI5)

**Analogy:** Imagine proving you know the solution to a Sudoku puzzle WITHOUT showing the solution.

- You prove: "I solved this correctly"
- They verify: "Yes, you did"
- They learn: NOTHING about your actual solution

**In code terms:**
```
Traditional:
  Input: [secret data]
  Output: [result]
  Trust: "Please believe I computed this correctly"

Zero-Knowledge:
  Input: [secret data]
  Output: [result] + [cryptographic proof]
  Trust: Math guarantees correctness. No trust needed.
```

### What is a zkVM? (Zero-Knowledge Virtual Machine)

A **zkVM** is a virtual computer that:
1. Runs your program
2. Automatically creates a proof that the program ran correctly

```
Normal Computer:              zkVM (like RISC Zero):
┌──────────────────┐          ┌──────────────────┐
│ Run program      │          │ Run program      │
│ Get result       │          │ Get result       │
│                  │          │ + GET PROOF      │
└──────────────────┘          └──────────────────┘
```

**Why is this powerful?**
- Write normal Rust code (not special cryptography code)
- The zkVM handles all the proof generation automatically
- Anyone can verify the proof in milliseconds

### What is Proof Generation?

**Proof generation** = creating the cryptographic proof that a computation was done correctly.

**Why is it slow?**
```
Regular computation:
  Run fraud detection on 1000 txs → 1 second

Proof generation:
  Same computation + create proof → 5-15 minutes
```

**Why the difference?**
- Every operation (add, multiply, memory read) must be recorded
- Mathematical constraints are created for each operation
- Complex cryptography turns constraints into a compact proof

It's like:
- **Solving** a puzzle = easy
- **Proving you solved it** without showing the answer = hard

### What is RISC Zero?

RISC Zero is the zkVM we use. It's made by a company called RISC Zero.

- **RISC-V:** A standard CPU instruction set (like x86 or ARM)
- **Zero:** Zero-knowledge proofs
- **Result:** Run standard Rust code, get proofs automatically

### What is Bonsai?

Bonsai is RISC Zero's **cloud proving service**.

```
Local proving:     Bonsai:
Your computer      RISC Zero's servers
Slow (10 min)      Fast (30 sec)
Free               Costs money
```

Like rendering video on your laptop vs. uploading to a render farm.

---

## 🏢 About World / Tools for Humanity

### What is World?
- Building a "proof of personhood" system
- Uses **Orb** device to scan irises (unique like fingerprints)
- Issues **World ID** - cryptographic proof you're human
- Privacy-preserving: proves you're human without revealing WHO you are

### Why Does This Matter?
As AI gets better:
- Bots flood the internet
- Fake accounts everywhere
- Hard to know if you're talking to a human

World ID = "I'm verified human #12345" (not "I'm John Smith")

### Key Stats (Mention in Interview!)
- 17+ million verified users
- 160 countries
- 350,000+ verifications per week
- Backed by a16z, Khosla, Tiger Global
- Featured on TIME Magazine cover

### The Detection & Response Team
Their job:
- Detect attacks on the World ID system
- Catch fake irises, Sybil attacks, operator fraud
- Do this WITHOUT exposing user biometric data

**The Challenge:** How do you run detection algorithms on sensitive data AND prove you did it correctly AND not expose the data?

**The Answer:** Verifiable compute with zkVMs! (What you built!)

---

## 💻 Your Project: World ZK Compute

### What It Does (Simple Version)
```
1. User posts job: "Run this fraud detection on this data. Here's $10 bounty."
2. Prover picks it up: "I'll do it!"
3. Prover runs it in zkVM: Gets result + proof
4. Prover submits proof on-chain
5. Smart contract verifies: "Proof is valid. Here's $10."
```

### Architecture
```
┌─────────────────────────────────────────────────────────┐
│                    SMART CONTRACTS                       │
│  ExecutionEngine: Manages jobs, bounties, verification  │
│  ProgramRegistry: Stores approved programs              │
└─────────────────────────────────────────────────────────┘
                          ↑ ↓
┌─────────────────────────────────────────────────────────┐
│                     PROVER NODE                          │
│  - Detects new jobs (WebSocket events)                  │
│  - Fetches inputs (with prefetching)                    │
│  - Runs zkVM (Bonsai/GPU/CPU)                           │
│  - Submits proofs (with retry logic)                    │
│  - Caches proofs for repeated requests                  │
└─────────────────────────────────────────────────────────┘
                          ↑ ↓
┌─────────────────────────────────────────────────────────┐
│                    RISC ZERO zkVM                        │
│  - Executes detection algorithms                        │
│  - Generates cryptographic proofs                       │
└─────────────────────────────────────────────────────────┘
```

### Key Optimizations You Built

| Problem | Solution | Improvement |
|---------|----------|-------------|
| Proofs took 10+ minutes | Bonsai cloud + GPU fallback | **20x faster** |
| 5-second job detection delay | WebSocket event subscription | **25x faster** |
| Input fetch blocked proving | Background prefetching | **20-50% faster** |
| Re-proved same computations | Proof caching | **Instant on repeat** |
| Parallel tx nonce conflicts | Atomic nonce manager | **Zero conflicts** |
| Transactions failed | Retry with exponential backoff | **98% success** |

---

## 💬 Expected Questions & Answers

### 1. "Tell me about yourself"

> "I'm a software engineer focused on blockchain and cryptography. Recently I've been building verifiable computation systems - specifically, a platform where smart contracts incentivize anyone to run programs in a zkVM and prove the results on-chain.
>
> I built this project called World ZK Compute, which is actually exactly what your job description mentions - 'blockchain-based verifiable compute for detection engineering.' I used RISC Zero for the zkVM, Rust for the prover, and Solidity for contracts.
>
> What excites me about this role is applying verifiable compute to security - detecting attacks on World ID while preserving user privacy."

---

### 2. "Why Tools for Humanity / World?"

> "Three reasons:
>
> **Mission:** As AI gets better at faking humanity, proving you're real becomes critical infrastructure. World is solving this at scale.
>
> **Technical challenge:** Decentralized detection that's both transparent AND privacy-preserving? That's cutting-edge cryptography meeting real-world security.
>
> **Alignment:** I've already built verifiable compute systems using RISC Zero. Your job description literally describes what I built. It feels like a perfect fit."

---

### 3. "Explain your project in simple terms"

> "Sure. Imagine you want someone to run a fraud detection algorithm on your data. How do you know they actually ran it correctly? Traditionally, you just have to trust them.
>
> My system solves this. You post a job to a smart contract with a bounty. Anyone can pick it up, run the algorithm in a special virtual machine called a zkVM, which creates a cryptographic proof that the computation was done correctly. They submit the proof on-chain, the contract verifies it, and they get paid.
>
> The key insight: the proof is tiny (a few hundred bytes) and verifies instantly, even if the computation took minutes. So we trade expensive computation (done once by the prover) for cheap verification (done by everyone on-chain)."

---

### 4. "What's proof generation? Why is it slow?"

> "Proof generation is creating the cryptographic evidence that a computation ran correctly.
>
> It's slow because you're not just running the program - you're building a mathematical proof for EVERY operation. If your program does a million additions, you need to prove each one was done correctly.
>
> Think of it like the difference between solving a puzzle versus proving you solved it without showing the answer. The solving is easy; the proving is hard.
>
> That's why we implemented Bonsai cloud proving - they have specialized hardware that generates proofs 20x faster than a regular computer."

---

### 5. "What challenges did you face?"

**Pick 2-3:**

**Challenge 1: Slow Proofs**
> "Initial proofs took 10-15 minutes on CPU. Through profiling, I found RISC Zero's STARK proving is just computationally intensive. I implemented a fallback chain: try Bonsai cloud first (30 seconds), fall back to local GPU, then CPU. This gave 20x speedup while maintaining reliability if any service is down."

**Challenge 2: Nonce Conflicts**
> "When processing multiple jobs in parallel, transactions failed with 'nonce too low' errors. The issue: multiple async tasks asked the blockchain for the current nonce, got the same number, then raced to submit. I built a custom nonce manager using atomic operations that allocates unique nonces locally, with recovery logic if we get out of sync with the chain."

**Challenge 3: Polling Latency**
> "We polled for new jobs every 5 seconds - that's up to 5 seconds of unnecessary delay. I replaced it with WebSocket subscriptions to contract events. Now we're notified within ~100 milliseconds when a new job is posted. Combined with input prefetching, jobs start processing almost instantly."

---

### 6. "How would verifiable compute help with security/detection?"

> "Great question - this is exactly the use case I designed for.
>
> **Traditional detection:** Run algorithms internally, users trust your results.
>
> **Verifiable detection:** Run algorithms in a zkVM, publish proof on-chain.
>
> For World specifically: you have 17 million users' biometric data. You need to detect attacks like fake irises or Sybil accounts. But you can't expose the raw data.
>
> With verifiable compute:
> 1. Publish encrypted/hashed audit logs to blockchain
> 2. Anyone can run detection algorithms in a zkVM
> 3. They prove 'I found 5 suspicious accounts' without revealing which ones
> 4. The proof is verified on-chain
>
> Now detection is transparent, auditable, and privacy-preserving."

---

### 7. "What's your Rust experience?"

> "I've been writing Rust for [your timeframe]. The World ZK Compute prover is about 5,000 lines of Rust.
>
> Key things I've used:
> - **Async/await with Tokio** for concurrent job processing
> - **Arc and RwLock** for thread-safe shared state
> - **Atomic operations** for the nonce manager
> - **Error handling** with anyhow
> - **Serde** for serialization
>
> I also wrote guest programs - the code that runs inside the zkVM - in Rust, which has constraints like no standard library and limited memory."

---

### 8. "Do you have questions for me?"

**Ask 2-3:**

1. "The job description mentions this project is early stage. What's been built so far, and what would my first contributions look like?"

2. "Are you using RISC Zero, or evaluating other zkVMs like SP1 or Jolt?"

3. "What does success look like for this internship? What would you hope I accomplish?"

4. "How does the D&R team balance detection accuracy with privacy guarantees?"

5. "Is there potential for this to convert to full-time?"

---

## 🎤 Your 30-Second Pitch

If asked "Why should we hire you?" or as a closing statement:

> "I built a working verifiable compute system that does exactly what your job description asks for - smart contracts that incentivize provers to run code, prove correctness, and submit results on-chain.
>
> I used RISC Zero, wrote the prover in Rust, and solved real engineering challenges like parallel processing, proof optimization, and transaction reliability.
>
> I'm not just theoretically interested - I've shipped working code. I'd love to bring that hands-on experience to your Detection & Response team."

---

## ⚠️ Things to Avoid

1. **Don't pretend to know everything** - "I'm not sure, but I'd approach it by..." is fine
2. **Don't ramble** - Keep answers to 1-2 minutes
3. **Don't be negative** - About anything
4. **Don't read from notes** - Use them to prepare, not during the call
5. **Don't forget to listen** - It's a conversation

---

## 📋 Day-Of Checklist

- [ ] Test Zoom link 10 minutes early
- [ ] Quiet room, good lighting
- [ ] Water nearby
- [ ] This doc open (for reference, don't read from it)
- [ ] GitHub ready to share if asked
- [ ] Smile and show enthusiasm!

---

## 🔗 Quick Links

- **Your GitHub:** https://github.com/OE-GOD/world-zk-compute
- **World Website:** https://world.org
- **RISC Zero Docs:** https://dev.risczero.com

---

## 📊 Key Numbers to Remember

| Metric | Value |
|--------|-------|
| World verified users | 17+ million |
| Countries | 160 |
| Verifications/week | 350,000+ |
| Your proof speedup | 20x (Bonsai vs CPU) |
| Job detection speedup | 25x (events vs polling) |
| Your success rate | 98% (with retry logic) |

---

Good luck! You've got this - you literally built what they're hiring for! 🚀
