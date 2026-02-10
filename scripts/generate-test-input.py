#!/usr/bin/env python3
"""Generate serialized risc0 serde inputs for E2E test examples.

Usage:
    python3 generate-test-input.py anomaly-detector
    python3 generate-test-input.py signature-verified
    python3 generate-test-input.py sybil-detector

Outputs parseable lines:
    DATA_URL=data:application/octet-stream;base64,...
    INPUT_DIGEST=0x...
"""
import struct
import hashlib
import base64
import sys

# ═══════════════════════════════════════════════════════════════════════════════
# risc0 word-aligned serde helpers
#
# risc0's serde format serializes everything as u32 little-endian words.
# Each primitive is zero-extended or split into u32 words.
# ═══════════════════════════════════════════════════════════════════════════════

def write_u8(val):
    """u8 -> ONE u32 word (zero-extended)"""
    return struct.pack('<I', val & 0xFF)

def write_u32(val):
    """u32 -> ONE u32 word"""
    return struct.pack('<I', val & 0xFFFFFFFF)

def write_u64(val):
    """u64 -> TWO u32 words (low, high)"""
    low = val & 0xFFFFFFFF
    high = (val >> 32) & 0xFFFFFFFF
    return struct.pack('<II', low, high)

def write_i64(val):
    """i64 -> TWO u32 words (two's complement)"""
    if val < 0:
        val = val + (1 << 64)
    return write_u64(val)

def write_f64(val):
    """f64 -> reinterpret as u64 bits -> TWO u32 words"""
    bits = struct.unpack('<Q', struct.pack('<d', val))[0]
    return write_u64(bits)

def write_usize(val):
    """usize -> TWO u32 words (risc0 serde deserialize_u64)"""
    return write_u64(val)

def write_byte_array_32(data):
    """[u8; 32] -> 32 u32 words (each byte as separate u32)"""
    assert len(data) == 32, f"expected 32 bytes, got {len(data)}"
    result = b''
    for b in data:
        result += write_u8(b)
    return result

def write_byte_array_16(data):
    """[u8; 16] -> 16 u32 words"""
    assert len(data) == 16, f"expected 16 bytes, got {len(data)}"
    result = b''
    for b in data:
        result += write_u8(b)
    return result

def write_byte_array_20(data):
    """[u8; 20] -> 20 u32 words"""
    assert len(data) == 20, f"expected 20 bytes, got {len(data)}"
    result = b''
    for b in data:
        result += write_u8(b)
    return result

def write_vec_u8(data):
    """Vec<u8> -> ONE u32 word for length, then each byte as u32"""
    result = write_u32(len(data))
    for b in data:
        result += write_u8(b)
    return result

def write_vec_f64(values):
    """Vec<f64> -> ONE u32 word for length, then each f64 as TWO u32 words"""
    result = write_u32(len(values))
    for v in values:
        result += write_f64(v)
    return result

def write_vec_i64(values):
    """Vec<i64> -> ONE u32 word for length, then each i64 as TWO u32 words"""
    result = write_u32(len(values))
    for v in values:
        result += write_i64(v)
    return result

# ═══════════════════════════════════════════════════════════════════════════════
# Anomaly Detector Input Generation
#
# Struct layout (risc0 serde order matches Rust struct field order):
#   DetectionInput {
#       data_points: Vec<DataPoint>,   // Vec length + each DataPoint
#       threshold: f64,
#       params: DetectionParams,
#   }
#   DataPoint { id: [u8;32], features: Vec<f64>, timestamp: u64 }
#   DetectionParams { window_size: usize, min_cluster_size: usize, distance_threshold: f64 }
# ═══════════════════════════════════════════════════════════════════════════════

def generate_anomaly_detector_input():
    """Generate a DetectionInput with 5 data points (1 anomaly)."""
    threshold = 0.5

    data_points = [
        {"id": bytes(32), "features": [100.0, 105.0, 98.0], "timestamp": 1700000001},
        {"id": bytes([1] + [0]*31), "features": [102.0, 99.0, 103.0], "timestamp": 1700000002},
        {"id": bytes([2] + [0]*31), "features": [101.0, 100.0, 100.0], "timestamp": 1700000003},
        {"id": bytes([3] + [0]*31), "features": [99.0, 101.0, 102.0], "timestamp": 1700000004},
        {"id": bytes([0xFF] + [0]*31), "features": [300.0, 50.0, 400.0], "timestamp": 1700000005},
    ]

    params = {"window_size": 10, "min_cluster_size": 3, "distance_threshold": 2.0}

    buf = b''

    # data_points: Vec<DataPoint>
    buf += write_u32(len(data_points))
    for point in data_points:
        buf += write_byte_array_32(point["id"])
        buf += write_vec_f64(point["features"])
        buf += write_u64(point["timestamp"])

    # threshold: f64
    buf += write_f64(threshold)

    # params: DetectionParams
    buf += write_usize(params["window_size"])
    buf += write_usize(params["min_cluster_size"])
    buf += write_f64(params["distance_threshold"])

    return buf

# ═══════════════════════════════════════════════════════════════════════════════
# Signature-Verified Input Generation
#
# Struct layout:
#   SignedDetectionInput {
#       data_points: Vec<DataPoint>,   // Vec length + each DataPoint
#       threshold: f64,
#       signature: Vec<u8>,            // 64-byte ECDSA sig
#       signer_pubkey: Vec<u8>,        // 33-byte compressed pubkey
#       expected_signer: [u8; 20],     // Ethereum-style address
#   }
#   DataPoint { id: [u8;32], features: Vec<i64>, timestamp: u64 }
# ═══════════════════════════════════════════════════════════════════════════════

def generate_signature_verified_input():
    """Generate a SignedDetectionInput with valid ECDSA signature."""
    try:
        from ecdsa import SigningKey, SECP256k1
    except ImportError:
        print("ERROR: 'ecdsa' package required for signature-verified example", file=sys.stderr)
        print("Install with: pip install ecdsa", file=sys.stderr)
        sys.exit(1)

    # Generate secp256k1 keypair
    sk = SigningKey.generate(curve=SECP256k1)
    vk = sk.get_verifying_key()

    # Compressed public key (33 bytes)
    compressed_pubkey = vk.to_string("compressed")

    # Derive address using SHA-256 (matching guest code which uses SHA-256 not Keccak)
    # Guest: address = sha256(uncompressed_pubkey[1..])[12..32]
    uncompressed_pubkey = b'\x04' + vk.to_string("uncompressed")
    pubkey_hash = hashlib.sha256(uncompressed_pubkey[1:]).digest()
    eth_address = pubkey_hash[12:32]

    threshold = 0.5

    data_points = [
        {"id": bytes(32), "features": [100, 105, 98], "timestamp": 1700000001},
        {"id": bytes([1] + [0]*31), "features": [102, 99, 103], "timestamp": 1700000002},
        {"id": bytes([2] + [0]*31), "features": [101, 100, 100], "timestamp": 1700000003},
        {"id": bytes([3] + [0]*31), "features": [99, 101, 102], "timestamp": 1700000004},
        {"id": bytes([0xFF] + [0]*31), "features": [300, 50, 400], "timestamp": 1700000005},
    ]

    # Hash data the same way the guest does
    hasher = hashlib.sha256()
    for point in data_points:
        hasher.update(point["id"])
        for feature in point["features"]:
            hasher.update(struct.pack('<q', feature))  # i64 LE bytes
        hasher.update(struct.pack('<Q', point["timestamp"]))
    hasher.update(struct.pack('<d', threshold))
    data_hash = hasher.digest()

    # Sign: k256's verify() hashes the message internally with SHA-256,
    # so we pass data_hash as the message and let ecdsa lib hash it
    signature = sk.sign(data_hash, hashfunc=hashlib.sha256)

    # Serialize in risc0 word-aligned serde format
    buf = b''

    # 1. data_points: Vec<DataPoint>
    buf += write_u32(len(data_points))
    for point in data_points:
        buf += write_byte_array_32(point["id"])
        buf += write_vec_i64(point["features"])
        buf += write_u64(point["timestamp"])

    # 2. threshold: f64
    buf += write_f64(threshold)

    # 3. signature: Vec<u8>
    buf += write_vec_u8(signature)

    # 4. signer_pubkey: Vec<u8>
    buf += write_vec_u8(compressed_pubkey)

    # 5. expected_signer: [u8; 20]
    buf += write_byte_array_20(eth_address)

    return buf

# ═══════════════════════════════════════════════════════════════════════════════
# Sybil Detector Input Generation
#
# Struct layout:
#   SybilDetectionInput {
#       registrations: Vec<Registration>,
#       config: SybilConfig,
#   }
#   Registration {
#       id: [u8; 32],
#       orb_id: [u8; 16],
#       timestamp: u64,
#       geo_hash: u32,
#       session_duration_secs: u32,
#       iris_score: u32,
#       verification_latency_ms: u32,
#   }
#   SybilConfig {
#       time_window_secs: u64,
#       geo_proximity_threshold: u32,
#       min_cluster_size: u32,
#       velocity_threshold_kmh: u32,
#       min_session_duration_secs: u32,
#       max_registrations_per_orb_per_hour: u32,
#   }
# ═══════════════════════════════════════════════════════════════════════════════

def generate_sybil_detector_input():
    """Generate a SybilDetectionInput with 10 registrations (5 normal, 5 suspicious).

    Test scenario:
    - Registrations 0-4: Normal (spread over hours, different orbs, good scores)
    - Registrations 5-7: Temporal cluster (same orb, 30s apart, fast sessions)
    - Registration 8: Geographic impossibility (distant location, 30s after reg 7)
    - Registration 9: Session anomaly (2-second scan, low quality)
    """
    ORB_A = bytes([0xAA] + [0]*15)  # Orb A
    ORB_B = bytes([0xBB] + [0]*15)  # Orb B
    ORB_C = bytes([0xCC] + [0]*15)  # Orb C

    base_time = 1700000000

    registrations = [
        # ── Normal registrations (spread out, different orbs) ────────────────
        {
            "id": bytes([0x01] + [0]*31),
            "orb_id": ORB_A,
            "timestamp": base_time,
            "geo_hash": 1000,       # Location A
            "session_duration_secs": 45,
            "iris_score": 850,
            "verification_latency_ms": 2000,
        },
        {
            "id": bytes([0x02] + [0]*31),
            "orb_id": ORB_B,
            "timestamp": base_time + 7200,  # 2 hours later
            "geo_hash": 2000,       # Location B
            "session_duration_secs": 50,
            "iris_score": 900,
            "verification_latency_ms": 1800,
        },
        {
            "id": bytes([0x03] + [0]*31),
            "orb_id": ORB_C,
            "timestamp": base_time + 14400,  # 4 hours later
            "geo_hash": 3000,       # Location C
            "session_duration_secs": 40,
            "iris_score": 820,
            "verification_latency_ms": 2200,
        },
        {
            "id": bytes([0x04] + [0]*31),
            "orb_id": ORB_A,
            "timestamp": base_time + 21600,  # 6 hours later
            "geo_hash": 1005,       # Near Location A
            "session_duration_secs": 55,
            "iris_score": 870,
            "verification_latency_ms": 1900,
        },
        {
            "id": bytes([0x05] + [0]*31),
            "orb_id": ORB_B,
            "timestamp": base_time + 28800,  # 8 hours later
            "geo_hash": 2010,       # Near Location B
            "session_duration_secs": 42,
            "iris_score": 880,
            "verification_latency_ms": 2100,
        },
        # ── Temporal cluster at ORB_A (30s apart, fast sessions) ─────────────
        {
            "id": bytes([0xF1] + [0]*31),
            "orb_id": ORB_A,
            "timestamp": base_time + 36000,  # 10 hours later
            "geo_hash": 1000,       # Location A
            "session_duration_secs": 8,       # Suspiciously fast
            "iris_score": 650,
            "verification_latency_ms": 500,
        },
        {
            "id": bytes([0xF2] + [0]*31),
            "orb_id": ORB_A,
            "timestamp": base_time + 36030,  # 30s later
            "geo_hash": 1000,       # Same location
            "session_duration_secs": 7,       # Suspiciously fast
            "iris_score": 620,
            "verification_latency_ms": 400,
        },
        {
            "id": bytes([0xF3] + [0]*31),
            "orb_id": ORB_A,
            "timestamp": base_time + 36060,  # Another 30s later
            "geo_hash": 1000,       # Same location
            "session_duration_secs": 6,       # Suspiciously fast
            "iris_score": 600,
            "verification_latency_ms": 350,
        },
        # ── Geographic impossibility (distant location 30s after reg 7) ──────
        {
            "id": bytes([0xF4] + [0]*31),
            "orb_id": ORB_C,
            "timestamp": base_time + 36090,  # 30s after cluster
            "geo_hash": 9000,       # Very distant location (delta 8000 ≈ 8000km)
            "session_duration_secs": 5,       # Also fast session
            "iris_score": 550,
            "verification_latency_ms": 300,
        },
        # ── Session anomaly (2-second scan, very low quality) ───────────────
        {
            "id": bytes([0xF5] + [0]*31),
            "orb_id": ORB_B,
            "timestamp": base_time + 43200,  # 12 hours later
            "geo_hash": 2000,       # Location B
            "session_duration_secs": 2,       # Impossibly fast
            "iris_score": 200,                # Very low quality
            "verification_latency_ms": 100,
        },
    ]

    config = {
        "time_window_secs": 300,        # 5-minute window for temporal clustering
        "geo_proximity_threshold": 100,  # Geohash units considered "nearby"
        "min_cluster_size": 2,           # 2+ nearby registrations = cluster
        "velocity_threshold_kmh": 1000,  # > 1000 km/h = impossible travel
        "min_session_duration_secs": 15, # Minimum 15s for real iris scan
        "max_registrations_per_orb_per_hour": 5,  # Max 5 per orb per hour
    }

    buf = b''

    # registrations: Vec<Registration>
    buf += write_u32(len(registrations))
    for reg in registrations:
        buf += write_byte_array_32(reg["id"])
        buf += write_byte_array_16(reg["orb_id"])
        buf += write_u64(reg["timestamp"])
        buf += write_u32(reg["geo_hash"])
        buf += write_u32(reg["session_duration_secs"])
        buf += write_u32(reg["iris_score"])
        buf += write_u32(reg["verification_latency_ms"])

    # config: SybilConfig
    buf += write_u64(config["time_window_secs"])
    buf += write_u32(config["geo_proximity_threshold"])
    buf += write_u32(config["min_cluster_size"])
    buf += write_u32(config["velocity_threshold_kmh"])
    buf += write_u32(config["min_session_duration_secs"])
    buf += write_u32(config["max_registrations_per_orb_per_hour"])

    return buf

# ═══════════════════════════════════════════════════════════════════════════════
# Main
# ═══════════════════════════════════════════════════════════════════════════════

def main():
    if len(sys.argv) != 2 or sys.argv[1] not in ("anomaly-detector", "signature-verified", "sybil-detector"):
        print("Usage: python3 generate-test-input.py [anomaly-detector|signature-verified|sybil-detector]", file=sys.stderr)
        sys.exit(1)

    example = sys.argv[1]

    if example == "anomaly-detector":
        buf = generate_anomaly_detector_input()
    elif example == "signature-verified":
        buf = generate_signature_verified_input()
    else:
        buf = generate_sybil_detector_input()

    # Create data URL
    b64 = base64.b64encode(buf).decode()
    data_url = f"data:application/octet-stream;base64,{b64}"

    # Compute SHA-256 digest for on-chain input verification
    digest = hashlib.sha256(buf).hexdigest()

    # Output parseable lines for shell scripts
    print(f"DATA_URL={data_url}")
    print(f"INPUT_DIGEST=0x{digest}")

if __name__ == "__main__":
    main()
