# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in World ZK Compute, please report it responsibly. **Do not open a public GitHub issue for security vulnerabilities.**

### How to Report

Send an email to **security@worldcoin.org** with the following information:

- A description of the vulnerability
- Steps to reproduce the issue
- The affected component(s) and version(s)
- Any potential impact assessment
- Your suggested fix, if you have one

You may encrypt your report using our PGP key, available at:
https://worldcoin.org/.well-known/pgp-key.txt

### Response Timeline

| Step | Timeframe |
|------|-----------|
| Acknowledgement | Within 48 hours |
| Initial assessment | Within 7 days |
| Status update | Every 14 days until resolved |
| Fix release | Within 90 days of confirmed vulnerability |

We may request additional information or clarification during the process. We will coordinate with you on disclosure timing.

## Scope

The following components are in scope for security reports:

- **Smart contracts** -- Solidity contracts in `contracts/src/`, including verifiers, execution engine, and upgradeability logic
- **TEE enclave** -- Attestation verification, model execution, and cryptographic operations in `tee/enclave/`
- **Operator service** -- Job submission, proof management, and dispute logic in `services/operator/`
- **Prover** -- RISC Zero prover and guest programs in `prover/` and `programs/`
- **SDKs** -- Rust, Python, and TypeScript client libraries in `sdk/`
- **Cryptographic primitives** -- GKR, Hyrax, Groth16, Poseidon, and ABI encoding in `examples/xgboost-remainder/`

## Out of Scope

The following are **not** in scope:

- Testnet or devnet deployments (Sepolia, Anvil)
- Documentation errors or typos
- Issues in third-party dependencies (report these upstream, but let us know if they affect our usage)
- Denial-of-service attacks against public infrastructure
- Social engineering
- Gas optimization suggestions (not security)
- Issues requiring physical access

## Bug Bounty

We may offer bounties for qualifying vulnerability reports at our discretion. Bounty eligibility and amounts depend on the severity and impact of the reported vulnerability. Contact security@worldcoin.org for current program details.

## Supported Versions

Security updates are provided for the latest release on the `main` branch. Older versions are not supported. We recommend always running the latest version.

## Disclosure Policy

We follow a 90-day coordinated disclosure timeline. We ask that you:

1. Allow us reasonable time to investigate and address the issue before public disclosure.
2. Make a good-faith effort to avoid privacy violations, data destruction, and service disruption.
3. Do not exploit the vulnerability beyond what is necessary to demonstrate the issue.

We will not pursue legal action against good-faith security researchers. Credit will be given to reporters in the security advisory unless they prefer to remain anonymous.

## Known Security Considerations

See [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md) for the project threat model.
