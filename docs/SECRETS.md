# Secrets Management

How to store, encrypt, rotate, and revoke secrets for World ZK Compute production deployments.

---

## 1. Secret Inventory

Every deployment manages these secret categories:

| Secret | Env Var | Used By | Rotation Urgency |
|--------|---------|---------|------------------|
| Operator wallet private key | `OPERATOR_PRIVATE_KEY` | operator service | High -- controls on-chain funds |
| Enclave signing private key | `ENCLAVE_PRIVATE_KEY` | TEE enclave | High -- attestation forgery if leaked |
| Registry signing key (ECDSA) | `REGISTRY_SIGNING_KEY` | proof-registry | Medium -- proof receipt integrity |
| Admin contract owner key | `ADMIN_PRIVATE_KEY` | deploy scripts, admin-cli | Critical -- full contract control |
| Gateway API keys | `GATEWAY_API_KEYS` | gateway service | Low -- rate limiting only |
| Verifier API keys | `VERIFIER_API_KEYS` | verifier service | Low -- legacy auth |
| Indexer admin API key | `ADMIN_API_KEY` | indexer service | Low -- admin endpoints |
| RPC provider API key | `RPC_URL` (embedded) | operator, indexer, enclave | Medium -- billing exposure |
| Grafana admin password | `GRAFANA_PASSWORD` | monitoring stack | Low |
| Slack/PagerDuty webhook URL | `ALERT_CONFIG_JSON` | operator alerting | Low |

### Criticality Tiers

- **Tier 1 (Critical)**: `ADMIN_PRIVATE_KEY` -- contract owner. Compromise grants full control over enclave registration, verifier upgrades, and contract pausing. Must be a multisig in production.
- **Tier 2 (High)**: `OPERATOR_PRIVATE_KEY`, `ENCLAVE_PRIVATE_KEY` -- on-chain asset risk and attestation integrity. Store in HSM or KMS.
- **Tier 3 (Medium)**: `REGISTRY_SIGNING_KEY`, RPC API keys -- service integrity and billing. Encrypt at rest.
- **Tier 4 (Low)**: API keys for gateway/verifier/indexer, monitoring passwords -- rotate periodically.

---

## 2. Encryption at Rest with SOPS + age

We use [SOPS](https://github.com/getsops/sops) with [age](https://age-encryption.org/) encryption for env files. SOPS encrypts values while leaving keys readable, making diffs and reviews practical.

### Initial Setup

```bash
# Install tools
brew install sops age          # macOS
# apt install sops age         # Debian/Ubuntu

# Generate an age key pair (one per operator/team member)
age-keygen -o ~/.config/sops/age/keys.txt
# Public key printed to stdout, e.g.: age1abc123...

# Note the public key -- this goes into .sops.yaml
cat ~/.config/sops/age/keys.txt | grep "public key"
```

### Configure SOPS

Create `.sops.yaml` at the repository root:

```yaml
creation_rules:
  # Production secrets -- require 2-of-3 recipients
  - path_regex: deploy/.*\.env\.encrypted$
    age: >-
      age1<team-lead-pubkey>,
      age1<security-engineer-pubkey>,
      age1<devops-engineer-pubkey>

  # AWS KMS integration (recommended for CI/CD)
  - path_regex: deploy/.*\.env\.encrypted$
    kms: arn:aws:kms:us-east-1:123456789:key/mrk-abc123
    age: >-
      age1<team-lead-pubkey>
```

### Encrypt an Environment File

```bash
# Start from the plaintext example
cp deploy/production.env.example deploy/secrets.env

# Fill in real values
vim deploy/secrets.env

# Encrypt (uses .sops.yaml rules)
sops encrypt deploy/secrets.env > deploy/secrets.env.encrypted

# Remove the plaintext
rm deploy/secrets.env

# Verify: keys are readable, values are encrypted
cat deploy/secrets.env.encrypted
```

### Decrypt for Use

```bash
# Decrypt to stdout (pipe to docker compose)
sops decrypt deploy/secrets.env.encrypted > /tmp/secrets.env
docker compose --env-file /tmp/secrets.env up -d
rm /tmp/secrets.env

# Or use sops exec-env for zero-disk-exposure
sops exec-env deploy/secrets.env.encrypted 'docker compose up -d'

# Edit in place (decrypts, opens editor, re-encrypts)
sops deploy/secrets.env.encrypted
```

### CI/CD Decryption

In GitHub Actions, use `SOPS_AGE_KEY` secret:

```yaml
# .github/workflows/deploy.yml
jobs:
  deploy:
    steps:
      - name: Decrypt secrets
        env:
          SOPS_AGE_KEY: ${{ secrets.SOPS_AGE_KEY }}
        run: |
          sops decrypt deploy/secrets.env.encrypted > .env.production
          # Deploy using .env.production
          # ...
          rm .env.production
```

For AWS KMS, the runner's IAM role needs `kms:Decrypt` permission on the key ARN.

---

## 3. AWS KMS Integration

For production deployments on AWS, KMS provides hardware-backed key management with audit trails via CloudTrail.

### KMS Key Setup

```bash
# Create a customer-managed key for secret encryption
aws kms create-key \
  --description "world-zk-compute secrets encryption" \
  --key-usage ENCRYPT_DECRYPT \
  --origin AWS_KMS \
  --tags TagKey=Project,TagValue=world-zk-compute

# Create an alias
aws kms create-alias \
  --alias-name alias/worldzk-secrets \
  --target-key-id <key-id>

# Grant decrypt to the operator IAM role
aws kms create-grant \
  --key-id <key-id> \
  --grantee-principal arn:aws:iam::123456789:role/worldzk-operator \
  --operations Decrypt
```

### SOPS with KMS

```yaml
# .sops.yaml with KMS
creation_rules:
  - path_regex: deploy/.*\.env\.encrypted$
    kms: arn:aws:kms:us-east-1:123456789:key/<key-id>
```

SOPS encrypts a data key with KMS. Decryption requires the IAM role to have `kms:Decrypt`. CloudTrail logs every decrypt call with caller identity and timestamp.

### Operator Private Key in KMS

For the highest-value secret (`OPERATOR_PRIVATE_KEY`), consider storing it directly in AWS Secrets Manager backed by KMS:

```bash
# Store the operator key
aws secretsmanager create-secret \
  --name worldzk/operator-private-key \
  --secret-string "0xabc123..." \
  --kms-key-id alias/worldzk-secrets \
  --tags Key=Project,Value=world-zk-compute

# Retrieve at runtime (in operator startup script)
OPERATOR_PRIVATE_KEY=$(aws secretsmanager get-secret-value \
  --secret-id worldzk/operator-private-key \
  --query SecretString --output text)
export OPERATOR_PRIVATE_KEY
```

### Key Policy Best Practices

```json
{
  "Sid": "AllowOperatorDecryptOnly",
  "Effect": "Allow",
  "Principal": {"AWS": "arn:aws:iam::123456789:role/worldzk-operator"},
  "Action": ["kms:Decrypt"],
  "Resource": "*",
  "Condition": {
    "StringEquals": {
      "kms:EncryptionContext:service": "world-zk-compute"
    }
  }
}
```

---

## 4. Key Rotation Procedures

### 4.1 Registry Signing Key (ECDSA)

The `REGISTRY_SIGNING_KEY` is a secp256k1 key used by the proof-registry service to sign proof receipts. If unset, the service auto-generates an ephemeral key on startup.

**When to rotate**: Quarterly, or immediately if the key may have been exposed.

```bash
# Generate a new signing key
./scripts/rotate-keys.sh --registry

# This outputs:
#   NEW_REGISTRY_SIGNING_KEY=<hex>
#   NEW_REGISTRY_PUBLIC_KEY=<hex>

# Update the encrypted env file
sops deploy/secrets.env.encrypted
# Replace REGISTRY_SIGNING_KEY with the new value

# Restart the proof-registry service
systemctl restart proof-registry
# or: kubectl rollout restart deployment/proof-registry

# Verify: check the /info endpoint returns the new public key
curl -s http://localhost:3001/info | jq .signing_public_key
```

**Impact**: Old proof receipts signed with the previous key remain valid (verification uses the public key embedded in the receipt). New receipts will use the new key.

### 4.2 API Keys (Gateway, Verifier, Indexer)

API keys are comma-separated strings. Rotation is additive: add the new key first, then remove the old one after all clients have migrated.

```bash
# Generate new API keys
./scripts/rotate-keys.sh --api-keys

# Rolling update procedure:
# 1. Add new key alongside old key
GATEWAY_API_KEYS="old-key-abc,new-key-xyz"
VERIFIER_API_KEYS="old-key-def,new-key-uvw"
ADMIN_API_KEY="new-admin-key-123"

# 2. Deploy with both keys active
sops deploy/secrets.env.encrypted   # update values
kubectl rollout restart deployment/gateway
kubectl rollout restart deployment/verifier
kubectl rollout restart deployment/indexer

# 3. Update all API clients to use new keys

# 4. Remove old keys
GATEWAY_API_KEYS="new-key-xyz"
VERIFIER_API_KEYS="new-key-uvw"

# 5. Redeploy
```

**Impact**: Zero downtime if done as a rolling update (steps 1-4).

### 4.3 Operator Private Key (Ethereum)

The operator key holds ETH for gas and submits on-chain transactions. Rotation requires transferring on-chain roles.

**When to rotate**: If the key may have been compromised, if a team member with access leaves, or annually.

```bash
# 1. Generate a new operator wallet
cast wallet new
# Save the new private key and address

# 2. Fund the new wallet
cast send <NEW_OPERATOR_ADDRESS> --value 0.5ether \
  --private-key $OLD_OPERATOR_PRIVATE_KEY \
  --rpc-url $RPC_URL

# 3. If the operator address has any on-chain roles (e.g., whitelisted submitter),
#    update those roles using the admin key:
cast send $TEE_VERIFIER_ADDRESS "setAuthorizedSubmitter(address,bool)" \
  $NEW_OPERATOR_ADDRESS true \
  --private-key $ADMIN_PRIVATE_KEY --rpc-url $RPC_URL

# 4. Update secrets
sops deploy/secrets.env.encrypted
# Replace OPERATOR_PRIVATE_KEY with the new key

# 5. Restart operator
systemctl restart tee-operator

# 6. Verify: check operator health and that it can submit transactions
curl -s http://localhost:9090/health | jq .
cast balance $NEW_OPERATOR_ADDRESS --rpc-url $RPC_URL

# 7. Revoke old operator (if it had on-chain roles)
cast send $TEE_VERIFIER_ADDRESS "setAuthorizedSubmitter(address,bool)" \
  $OLD_OPERATOR_ADDRESS false \
  --private-key $ADMIN_PRIVATE_KEY --rpc-url $RPC_URL

# 8. Drain remaining funds from old wallet
cast send $NEW_OPERATOR_ADDRESS --value $(cast balance $OLD_OPERATOR_ADDRESS --rpc-url $RPC_URL) \
  --private-key $OLD_OPERATOR_PRIVATE_KEY --rpc-url $RPC_URL
```

### 4.4 Enclave Signing Key

See `scripts/rotate-enclave-key.sh` for the full procedure. Summary:

```bash
# 1. Launch new enclave instance (auto-generates key on startup)
# 2. Get new enclave address from /info endpoint
# 3. Register on-chain: registerEnclave(newAddr, pcr0Hash)
# 4. Verify: cast call $VERIFIER "enclaves(address)" $NEW_ADDR
# 5. Revoke old enclave: revokeEnclave(oldAddr)
# 6. Update ENCLAVE_URL to point to new enclave
```

### 4.5 Admin / Contract Owner Key

The admin key is the highest-value secret. It controls contract upgrades, enclave registration, and emergency pause.

**Production requirement**: Use a multisig (Gnosis Safe) as contract owner. Never use a single EOA in production.

```bash
# Transfer ownership to a multisig (one-time setup)
cast send $TEE_VERIFIER_ADDRESS "transferOwnership(address)" \
  $MULTISIG_ADDRESS \
  --private-key $ADMIN_PRIVATE_KEY --rpc-url $RPC_URL

# The new owner must accept (Ownable2Step):
# Submit this from the multisig:
# TEEMLVerifier.acceptOwnership()
```

After transferring to a multisig, "rotation" means changing the multisig signers via the Safe UI, not changing the contract owner address.

---

## 5. Emergency Key Revocation

### Enclave Key Compromise

If a TEE signing key is suspected compromised:

```bash
# 1. IMMEDIATELY revoke the enclave on-chain
cast send $TEE_VERIFIER_ADDRESS "revokeEnclave(address)" \
  $COMPROMISED_ENCLAVE_ADDRESS \
  --private-key $ADMIN_PRIVATE_KEY --rpc-url $RPC_URL

# 2. Pause contract if the attacker is actively submitting bad results
cast send $TEE_VERIFIER_ADDRESS "pause()" \
  --private-key $ADMIN_PRIVATE_KEY --rpc-url $RPC_URL

# 3. Challenge all pending results from the compromised enclave
#    (within the 1-hour challenge window)
#    Use the operator's challenge functionality or manual cast calls:
cast send $TEE_VERIFIER_ADDRESS "challenge(bytes32)" \
  $RESULT_ID \
  --value 0.1ether \
  --private-key $CHALLENGER_PRIVATE_KEY --rpc-url $RPC_URL

# 4. Deploy new enclave, register new key (see Section 4.4)

# 5. Unpause after new enclave is verified
cast send $TEE_VERIFIER_ADDRESS "unpause()" \
  --private-key $ADMIN_PRIVATE_KEY --rpc-url $RPC_URL
```

### Operator Key Compromise

```bash
# 1. Drain funds from compromised wallet to a safe address
cast send $SAFE_ADDRESS --value $(cast balance $COMPROMISED --rpc-url $RPC) \
  --private-key $COMPROMISED_KEY --rpc-url $RPC

# 2. Revoke on-chain roles (if any)
cast send $TEE_VERIFIER_ADDRESS "setAuthorizedSubmitter(address,bool)" \
  $COMPROMISED_ADDRESS false \
  --private-key $ADMIN_PRIVATE_KEY --rpc-url $RPC_URL

# 3. Generate and fund new operator wallet (see Section 4.3)
```

### Admin Key Compromise

This is the worst-case scenario. If the admin key is a single EOA:

```bash
# Race condition: try to transfer ownership before the attacker does
cast send $TEE_VERIFIER_ADDRESS "transferOwnership(address)" \
  $EMERGENCY_MULTISIG_ADDRESS \
  --private-key $ADMIN_PRIVATE_KEY --rpc-url $RPC_URL \
  --gas-price $(cast gas-price --rpc-url $RPC_URL | awk '{print $1 * 2}')

# If the attacker already changed ownership: the contract is lost.
# Deploy fresh contracts and migrate. This is why production MUST use a multisig.
```

### Revocation Checklist

- [ ] Identify which key was compromised (enclave, operator, admin, API)
- [ ] Revoke the compromised key/address on-chain immediately
- [ ] Pause contracts if active exploitation is occurring
- [ ] Challenge any pending suspicious results within the challenge window
- [ ] Rotate the compromised key (see Section 4)
- [ ] Notify the team via the incident template (see `docs/DISASTER_RECOVERY.md` Section 7)
- [ ] Post-incident: audit CloudTrail/logs, conduct root cause analysis

---

## 6. Environment File Lifecycle

### Development

```
.env.example          <-- committed, placeholder values
.env                  <-- gitignored, developer fills in locally
```

### Staging / CI

```
deploy/secrets.env.encrypted     <-- committed, SOPS-encrypted
.github/secrets/SOPS_AGE_KEY    <-- GitHub Actions secret (decrypts at deploy time)
```

### Production

```
deploy/secrets.env.encrypted         <-- committed, SOPS+KMS encrypted
AWS Secrets Manager / Vault          <-- Tier 1-2 keys (ADMIN, OPERATOR, ENCLAVE)
deploy/production.env.example        <-- committed, documents all vars
```

### Security Rules

1. **Never commit plaintext secrets** to version control. The `.gitignore` already excludes `.env`, `.env.local`, `.env.sepolia`, `*.key`, `*.pem`.
2. **Never log secret values**. The operator uses `SecretString` for `OPERATOR_PRIVATE_KEY` to prevent accidental logging.
3. **Restrict file permissions**: `chmod 600` on all `.env` files. SOPS-encrypted files can be `644` since values are ciphertext.
4. **Use short-lived credentials** where possible: AWS IAM roles (auto-rotated), short-lived API tokens.
5. **Audit access**: Enable CloudTrail for KMS decrypt events. Review access logs quarterly.

---

## 7. SOPS Quick Reference

| Operation | Command |
|-----------|---------|
| Encrypt a file | `sops encrypt secrets.env > secrets.env.encrypted` |
| Decrypt to stdout | `sops decrypt secrets.env.encrypted` |
| Decrypt to file | `sops decrypt secrets.env.encrypted > secrets.env` |
| Edit in place | `sops secrets.env.encrypted` |
| Add a recipient | `sops updatekeys secrets.env.encrypted` (after updating `.sops.yaml`) |
| Rotate data key | `sops rotate -i secrets.env.encrypted` |
| Run command with decrypted env | `sops exec-env secrets.env.encrypted 'command'` |

---

## 8. Rotation Schedule

| Secret | Rotation Period | Automated? | Notes |
|--------|----------------|------------|-------|
| Registry signing key | Quarterly | `rotate-keys.sh --registry` | Zero-downtime |
| API keys (gateway/verifier) | Quarterly | `rotate-keys.sh --api-keys` | Rolling update |
| Indexer admin API key | Quarterly | `rotate-keys.sh --api-keys` | Single key, brief restart |
| Operator private key | Annually or on personnel change | Manual | Requires fund transfer |
| Enclave signing key | On enclave rebuild | `rotate-enclave-key.sh` | On-chain registration needed |
| Admin / owner key | N/A (multisig signers rotate) | Manual | Gnosis Safe signer management |
| RPC provider API key | Per provider policy | Manual | Update in encrypted env |
| SOPS data key | Annually | `sops rotate -i` | Transparent to services |
| KMS key | Per AWS policy (auto-rotation available) | AWS-managed | Enable annual auto-rotation |
