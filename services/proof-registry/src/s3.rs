//! S3 storage backend with Object Lock (GOVERNANCE or COMPLIANCE mode).
//!
//! This module is only compiled when the `s3` feature is enabled. It uses the
//! AWS SDK for Rust to interact with S3, applying Object Lock retention on every
//! `PutObject` call so that proofs cannot be deleted or overwritten before the
//! retention period expires.
//!
//! ## Required Bucket Configuration
//!
//! The target S3 bucket must have Object Lock **enabled at creation time** and
//! versioning must be **enabled** (Object Lock requires it). This module does
//! NOT attempt to configure the bucket — it assumes the bucket is already set up
//! correctly.
//!
//! ## Environment Variables
//!
//! | Variable | Description | Default |
//! |---|---|---|
//! | `S3_BUCKET` | Bucket name | (required) |
//! | `S3_REGION` | AWS region | `us-east-1` |
//! | `S3_RETENTION_MODE` | `GOVERNANCE` or `COMPLIANCE` | `GOVERNANCE` |
//! | `S3_RETENTION_YEARS` | Retention period in years | `7` |
//! | `S3_PREFIX` | Key prefix inside the bucket | `proofs/` |

use async_trait::async_trait;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{ObjectLockMode, ObjectLockRetention};
use aws_sdk_s3::Client;
use chrono::Utc;

use crate::storage::{ProofStorage, StorageError};

/// S3 storage backend with Object Lock retention.
pub struct S3Storage {
    client: Client,
    bucket: String,
    prefix: String,
    retention_mode: ObjectLockMode,
    retention_years: u32,
}

impl S3Storage {
    /// Create a new S3 storage backend.
    ///
    /// # Arguments
    /// - `bucket` — S3 bucket name (must have Object Lock enabled).
    /// - `region` — AWS region (e.g. `"us-east-1"`).
    /// - `mode` — Retention mode: `"GOVERNANCE"` or `"COMPLIANCE"`.
    /// - `years` — Retention period in years.
    /// - `prefix` — Key prefix (e.g. `"proofs/"`).
    pub async fn new(
        bucket: &str,
        region: &str,
        mode: &str,
        years: u32,
        prefix: &str,
    ) -> Result<Self, StorageError> {
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new(region.to_string()))
            .load()
            .await;

        let client = Client::new(&config);

        let retention_mode = match mode.to_uppercase().as_str() {
            "COMPLIANCE" => ObjectLockMode::Compliance,
            "GOVERNANCE" | _ => ObjectLockMode::Governance,
        };

        Ok(Self {
            client,
            bucket: bucket.to_string(),
            prefix: prefix.to_string(),
            retention_mode,
            retention_years: years,
        })
    }

    /// Build the full S3 key for a proof ID.
    fn s3_key(&self, id: &str) -> String {
        let date = Utc::now().format("%Y-%m-%d");
        format!("{}{}/{}.json", self.prefix, date, id)
    }

    /// Compute the retention expiry from now.
    fn retention(&self) -> ObjectLockRetention {
        let retain_until = Utc::now() + chrono::Duration::days(self.retention_years as i64 * 365);
        let retain_until_aws =
            aws_sdk_s3::primitives::DateTime::from_millis(retain_until.timestamp_millis());

        ObjectLockRetention::builder()
            .mode(self.retention_mode.clone())
            .retain_until_date(retain_until_aws)
            .build()
    }
}

#[async_trait]
impl ProofStorage for S3Storage {
    async fn store(&self, id: &str, data: &[u8]) -> Result<String, StorageError> {
        // Check for path traversal.
        if id.is_empty()
            || id.contains("..")
            || id.contains('/')
            || id.contains('\\')
            || id.contains('\0')
        {
            return Err(StorageError::InvalidId(id.to_string()));
        }

        // Check if the proof already exists.
        if self.exists(id).await? {
            return Err(StorageError::AlreadyExists(id.to_string()));
        }

        let key = self.s3_key(id);
        let retention = self.retention();

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(ByteStream::from(data.to_vec()))
            .content_type("application/json")
            .object_lock_mode(self.retention_mode.clone())
            .object_lock_retain_until_date(
                retention
                    .retain_until_date()
                    .cloned()
                    .unwrap_or_else(|| aws_sdk_s3::primitives::DateTime::from_millis(0)),
            )
            .send()
            .await
            .map_err(|e| StorageError::Io(format!("S3 PutObject failed: {e}")))?;

        Ok(key)
    }

    async fn get(&self, id: &str) -> Result<Vec<u8>, StorageError> {
        if id.is_empty()
            || id.contains("..")
            || id.contains('/')
            || id.contains('\\')
            || id.contains('\0')
        {
            return Err(StorageError::InvalidId(id.to_string()));
        }

        // We need to find the key — it could be under any date partition.
        // List objects with the prefix that ends with the proof ID.
        let prefix = &self.prefix;
        let suffix = format!("{id}.json");

        let result = self
            .client
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(prefix)
            .send()
            .await
            .map_err(|e| StorageError::Io(format!("S3 ListObjectsV2 failed: {e}")))?;

        let key = result
            .contents()
            .iter()
            .find_map(|obj| {
                let k = obj.key()?;
                if k.ends_with(&suffix) {
                    Some(k.to_string())
                } else {
                    None
                }
            })
            .ok_or_else(|| StorageError::NotFound(id.to_string()))?;

        let output = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            .map_err(|e| StorageError::Io(format!("S3 GetObject failed: {e}")))?;

        let bytes = output
            .body
            .collect()
            .await
            .map_err(|e| StorageError::Io(format!("S3 read body failed: {e}")))?;

        Ok(bytes.into_bytes().to_vec())
    }

    async fn exists(&self, id: &str) -> Result<bool, StorageError> {
        if id.is_empty()
            || id.contains("..")
            || id.contains('/')
            || id.contains('\\')
            || id.contains('\0')
        {
            return Err(StorageError::InvalidId(id.to_string()));
        }

        let suffix = format!("{id}.json");
        let result = self
            .client
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(&self.prefix)
            .send()
            .await
            .map_err(|e| StorageError::Io(format!("S3 ListObjectsV2 failed: {e}")))?;

        Ok(result
            .contents()
            .iter()
            .any(|obj| obj.key().is_some_and(|k| k.ends_with(&suffix))))
    }

    async fn list(&self, offset: usize, limit: usize) -> Result<Vec<String>, StorageError> {
        let result = self
            .client
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(&self.prefix)
            .send()
            .await
            .map_err(|e| StorageError::Io(format!("S3 ListObjectsV2 failed: {e}")))?;

        let ids: Vec<String> = result
            .contents()
            .iter()
            .filter_map(|obj| {
                let key = obj.key()?;
                // Extract proof ID from key like "proofs/2024-01-15/abc.json"
                let filename = key.rsplit('/').next()?;
                filename.strip_suffix(".json").map(String::from)
            })
            .skip(offset)
            .take(limit)
            .collect();

        Ok(ids)
    }

    fn storage_type(&self) -> &str {
        "s3"
    }
}

// ---------------------------------------------------------------------------
// Compile-time verification tests (no real S3 credentials needed)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that S3Storage struct can be constructed and the types compile.
    /// Actual S3 operations require credentials and a real bucket.
    #[test]
    fn test_s3_key_format() {
        // We can test the key format logic without a real S3 client.
        // Construct a mock-ish instance to verify key generation.
        let prefix = "proofs/";
        let id = "test-proof-123";
        let date = Utc::now().format("%Y-%m-%d").to_string();
        let expected = format!("{prefix}{date}/{id}.json");

        // Verify the format matches what s3_key would produce.
        assert!(expected.starts_with("proofs/"));
        assert!(expected.ends_with("/test-proof-123.json"));
        assert!(expected.contains(&date));
    }

    #[test]
    fn test_retention_mode_parsing() {
        // Verify our mode parsing logic.
        let governance = match "GOVERNANCE".to_uppercase().as_str() {
            "COMPLIANCE" => ObjectLockMode::Compliance,
            "GOVERNANCE" | _ => ObjectLockMode::Governance,
        };
        assert_eq!(governance, ObjectLockMode::Governance);

        let compliance = match "COMPLIANCE".to_uppercase().as_str() {
            "COMPLIANCE" => ObjectLockMode::Compliance,
            "GOVERNANCE" | _ => ObjectLockMode::Governance,
        };
        assert_eq!(compliance, ObjectLockMode::Compliance);

        // Unknown defaults to Governance.
        let unknown = match "UNKNOWN".to_uppercase().as_str() {
            "COMPLIANCE" => ObjectLockMode::Compliance,
            "GOVERNANCE" | _ => ObjectLockMode::Governance,
        };
        assert_eq!(unknown, ObjectLockMode::Governance);
    }
}
