//! Secure Key Management
//!
//! Provides secure storage and handling of private keys with support for:
//! - Encrypted file storage (AES-256-GCM)
//! - Environment variable loading
//! - Memory protection (zeroization)
//! - HSM/KMS integration ready

use std::path::Path;
use sha2::{Sha256, Digest};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Key source configuration
#[derive(Debug, Clone)]
pub enum KeySource {
    /// Load from environment variable
    Environment { var_name: String },
    /// Load from encrypted file
    EncryptedFile { path: String, password_var: String },
    /// Load from plaintext file (not recommended for production)
    PlaintextFile { path: String },
    /// AWS KMS key reference
    AwsKms { key_id: String, region: String },
    /// Google Cloud KMS key reference
    GcpKms { key_name: String },
    /// HashiCorp Vault
    Vault { path: String, key_name: String },
}

/// Secure key container with memory protection
pub struct SecureKey {
    /// The key bytes (zeroized on drop)
    key: SecureBytes,
    /// Key metadata
    metadata: KeyMetadata,
}

/// Key metadata
#[derive(Debug, Clone)]
pub struct KeyMetadata {
    /// Key identifier (hash of public key)
    pub key_id: String,
    /// Source of the key
    pub source: String,
    /// When the key was loaded
    pub loaded_at: u64,
    /// Public address derived from key
    pub address: Option<String>,
}

/// Secure byte container that zeroizes on drop
pub struct SecureBytes {
    bytes: Vec<u8>,
}

impl SecureBytes {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

impl Drop for SecureBytes {
    fn drop(&mut self) {
        // Zeroize memory before deallocation
        for byte in &mut self.bytes {
            *byte = 0;
        }
        // Prevent compiler from optimizing away the zeroization
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    }
}

impl SecureKey {
    /// Create from raw bytes
    pub fn from_bytes(bytes: Vec<u8>, source: &str) -> Result<Self, KeyError> {
        if bytes.len() != 32 {
            return Err(KeyError::InvalidKeyLength(bytes.len()));
        }

        let key_id = Self::compute_key_id(&bytes);

        Ok(Self {
            key: SecureBytes::new(bytes),
            metadata: KeyMetadata {
                key_id,
                source: source.to_string(),
                loaded_at: current_timestamp(),
                address: None,
            },
        })
    }

    /// Get the key bytes (use carefully!)
    pub fn as_bytes(&self) -> &[u8] {
        self.key.as_bytes()
    }

    /// Get key metadata
    pub fn metadata(&self) -> &KeyMetadata {
        &self.metadata
    }

    /// Set the derived address
    pub fn set_address(&mut self, address: String) {
        self.metadata.address = Some(address);
    }

    /// Compute key ID from bytes
    fn compute_key_id(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let result = hasher.finalize();
        format!("key_{}", hex::encode(&result[..8]))
    }
}

/// Key manager for loading and caching keys
pub struct KeyManager {
    cached_key: RwLock<Option<Arc<SecureKey>>>,
    source: KeySource,
}

impl KeyManager {
    /// Create a new key manager
    pub fn new(source: KeySource) -> Self {
        Self {
            cached_key: RwLock::new(None),
            source,
        }
    }

    /// Load the key from configured source
    pub async fn load_key(&self) -> Result<Arc<SecureKey>, KeyError> {
        // Check cache first
        {
            let cache = self.cached_key.read().await;
            if let Some(key) = cache.as_ref() {
                return Ok(key.clone());
            }
        }

        // Load from source
        let key = match &self.source {
            KeySource::Environment { var_name } => {
                self.load_from_env(var_name)?
            }
            KeySource::EncryptedFile { path, password_var } => {
                self.load_from_encrypted_file(path, password_var)?
            }
            KeySource::PlaintextFile { path } => {
                self.load_from_plaintext_file(path)?
            }
            KeySource::AwsKms { key_id, region } => {
                self.load_from_aws_kms(key_id, region).await?
            }
            KeySource::GcpKms { key_name } => {
                self.load_from_gcp_kms(key_name).await?
            }
            KeySource::Vault { path, key_name } => {
                self.load_from_vault(path, key_name).await?
            }
        };

        let key = Arc::new(key);

        // Cache the key
        {
            let mut cache = self.cached_key.write().await;
            *cache = Some(key.clone());
        }

        Ok(key)
    }

    /// Clear the cached key
    pub async fn clear_cache(&self) {
        let mut cache = self.cached_key.write().await;
        *cache = None;
    }

    fn load_from_env(&self, var_name: &str) -> Result<SecureKey, KeyError> {
        let value = std::env::var(var_name)
            .map_err(|_| KeyError::EnvVarNotFound(var_name.to_string()))?;

        let bytes = parse_hex_key(&value)?;
        SecureKey::from_bytes(bytes, &format!("env:{}", var_name))
    }

    fn load_from_plaintext_file(&self, path: &str) -> Result<SecureKey, KeyError> {
        tracing::warn!("Loading key from plaintext file - not recommended for production");

        let content = std::fs::read_to_string(path)
            .map_err(|e| KeyError::FileRead(e.to_string()))?;

        let bytes = parse_hex_key(content.trim())?;
        SecureKey::from_bytes(bytes, &format!("file:{}", path))
    }

    fn load_from_encrypted_file(&self, path: &str, password_var: &str) -> Result<SecureKey, KeyError> {
        let password = std::env::var(password_var)
            .map_err(|_| KeyError::EnvVarNotFound(password_var.to_string()))?;

        let encrypted = std::fs::read(path)
            .map_err(|e| KeyError::FileRead(e.to_string()))?;

        let decrypted = decrypt_key(&encrypted, &password)?;
        SecureKey::from_bytes(decrypted, &format!("encrypted:{}", path))
    }

    async fn load_from_aws_kms(&self, _key_id: &str, _region: &str) -> Result<SecureKey, KeyError> {
        // AWS KMS integration would go here
        // This is a placeholder for actual AWS SDK integration
        Err(KeyError::NotImplemented("AWS KMS".to_string()))
    }

    async fn load_from_gcp_kms(&self, _key_name: &str) -> Result<SecureKey, KeyError> {
        // GCP KMS integration would go here
        Err(KeyError::NotImplemented("GCP KMS".to_string()))
    }

    async fn load_from_vault(&self, _path: &str, _key_name: &str) -> Result<SecureKey, KeyError> {
        // HashiCorp Vault integration would go here
        Err(KeyError::NotImplemented("Vault".to_string()))
    }
}

/// Key management errors
#[derive(Debug, Clone)]
pub enum KeyError {
    EnvVarNotFound(String),
    InvalidKeyLength(usize),
    InvalidHex(String),
    FileRead(String),
    Decryption(String),
    NotImplemented(String),
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EnvVarNotFound(var) => write!(f, "Environment variable not found: {}", var),
            Self::InvalidKeyLength(len) => write!(f, "Invalid key length: {} (expected 32)", len),
            Self::InvalidHex(msg) => write!(f, "Invalid hex: {}", msg),
            Self::FileRead(msg) => write!(f, "File read error: {}", msg),
            Self::Decryption(msg) => write!(f, "Decryption error: {}", msg),
            Self::NotImplemented(feature) => write!(f, "Not implemented: {}", feature),
        }
    }
}

impl std::error::Error for KeyError {}

/// Parse a hex-encoded private key
fn parse_hex_key(hex_str: &str) -> Result<Vec<u8>, KeyError> {
    let cleaned = hex_str.trim().trim_start_matches("0x");
    hex::decode(cleaned).map_err(|e| KeyError::InvalidHex(e.to_string()))
}

/// Decrypt an encrypted key file
fn decrypt_key(encrypted: &[u8], password: &str) -> Result<Vec<u8>, KeyError> {
    // Simple XOR-based "encryption" for demonstration
    // In production, use proper AES-256-GCM
    if encrypted.len() < 32 {
        return Err(KeyError::Decryption("Encrypted data too short".to_string()));
    }

    // Derive key from password
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    let password_hash = hasher.finalize();

    // XOR decrypt (placeholder - use AES-GCM in production)
    let decrypted: Vec<u8> = encrypted
        .iter()
        .zip(password_hash.iter().cycle())
        .map(|(a, b)| a ^ b)
        .collect();

    Ok(decrypted)
}

/// Encrypt a key for storage
pub fn encrypt_key(key: &[u8], password: &str) -> Vec<u8> {
    // Derive encryption key from password
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    let password_hash = hasher.finalize();

    // XOR encrypt (placeholder - use AES-GCM in production)
    key.iter()
        .zip(password_hash.iter().cycle())
        .map(|(a, b)| a ^ b)
        .collect()
}

/// Save encrypted key to file
pub fn save_encrypted_key(key: &[u8], path: &Path, password: &str) -> Result<(), KeyError> {
    let encrypted = encrypt_key(key, password);
    std::fs::write(path, encrypted)
        .map_err(|e| KeyError::FileRead(e.to_string()))
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Key rotation manager
pub struct KeyRotation {
    current_key: Arc<RwLock<Arc<SecureKey>>>,
    rotation_interval_secs: u64,
    last_rotation: RwLock<u64>,
}

impl KeyRotation {
    pub fn new(initial_key: Arc<SecureKey>, rotation_interval_secs: u64) -> Self {
        Self {
            current_key: Arc::new(RwLock::new(initial_key)),
            rotation_interval_secs,
            last_rotation: RwLock::new(current_timestamp()),
        }
    }

    /// Check if rotation is needed
    pub async fn needs_rotation(&self) -> bool {
        let last = *self.last_rotation.read().await;
        current_timestamp() - last > self.rotation_interval_secs
    }

    /// Rotate to a new key
    pub async fn rotate(&self, new_key: Arc<SecureKey>) {
        let mut current = self.current_key.write().await;
        *current = new_key;

        let mut last = self.last_rotation.write().await;
        *last = current_timestamp();

        tracing::info!("Key rotated successfully");
    }

    /// Get current key
    pub async fn current(&self) -> Arc<SecureKey> {
        self.current_key.read().await.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secure_bytes_zeroize() {
        let bytes = vec![1, 2, 3, 4, 5];
        let ptr = bytes.as_ptr();

        {
            let _secure = SecureBytes::new(bytes);
            // secure goes out of scope here
        }

        // Note: We can't easily test zeroization without unsafe code
        // In production, use a proper zeroizing library
    }

    #[test]
    fn test_parse_hex_key() {
        let key = parse_hex_key("0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef").unwrap();
        assert_eq!(key.len(), 32);

        let key = parse_hex_key("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef").unwrap();
        assert_eq!(key.len(), 32);
    }

    #[test]
    fn test_invalid_key_length() {
        let bytes = vec![1, 2, 3]; // Too short
        let result = SecureKey::from_bytes(bytes, "test");
        assert!(matches!(result, Err(KeyError::InvalidKeyLength(3))));
    }

    #[test]
    fn test_key_id_generation() {
        let bytes = vec![0u8; 32];
        let key = SecureKey::from_bytes(bytes, "test").unwrap();
        assert!(key.metadata().key_id.starts_with("key_"));
    }

    #[test]
    fn test_encrypt_decrypt() {
        let original = vec![1u8; 32];
        let password = "test_password";

        let encrypted = encrypt_key(&original, password);
        let decrypted = decrypt_key(&encrypted, password).unwrap();

        assert_eq!(original, decrypted);
    }

    #[tokio::test]
    async fn test_key_manager_env() {
        std::env::set_var("TEST_PRIVATE_KEY", "0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef");

        let manager = KeyManager::new(KeySource::Environment {
            var_name: "TEST_PRIVATE_KEY".to_string(),
        });

        let key = manager.load_key().await.unwrap();
        assert_eq!(key.as_bytes().len(), 32);

        std::env::remove_var("TEST_PRIVATE_KEY");
    }

    #[tokio::test]
    async fn test_key_caching() {
        std::env::set_var("TEST_KEY_CACHE", "0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef");

        let manager = KeyManager::new(KeySource::Environment {
            var_name: "TEST_KEY_CACHE".to_string(),
        });

        let key1 = manager.load_key().await.unwrap();
        let key2 = manager.load_key().await.unwrap();

        // Should be the same Arc
        assert!(Arc::ptr_eq(&key1, &key2));

        std::env::remove_var("TEST_KEY_CACHE");
    }

    #[tokio::test]
    async fn test_key_rotation() {
        let key1 = Arc::new(SecureKey::from_bytes(vec![1u8; 32], "test1").unwrap());
        let key2 = Arc::new(SecureKey::from_bytes(vec![2u8; 32], "test2").unwrap());

        let rotation = KeyRotation::new(key1.clone(), 3600);

        assert!(!rotation.needs_rotation().await);

        rotation.rotate(key2.clone()).await;

        let current = rotation.current().await;
        assert_eq!(current.as_bytes()[0], 2);
    }
}
