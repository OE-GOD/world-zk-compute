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
//! | `S3_RETENTION_MODE` | `GOVERNANCE` or `COMPLIANCE`; any other value is an error | `GOVERNANCE` |
//! | `S3_RETENTION_YEARS` | Retention period in calendar years (a whole number above 0); any other value is an error | `7` |
//! | `S3_PREFIX` | Key prefix inside the bucket | `proofs/` |
//!
//! A value that is not valid UTF-8 is an error for each of them.

use std::ffi::{OsStr, OsString};

use async_trait::async_trait;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{Object, ObjectLockMode};
use aws_sdk_s3::Client;
use chrono::{DateTime, Months, Utc};

use crate::storage::{ProofStorage, StorageError};

/// Parse a retention mode (case-insensitive).
fn parse_retention_mode(mode: &str) -> Result<ObjectLockMode, StorageError> {
    match mode.to_uppercase().as_str() {
        "GOVERNANCE" => Ok(ObjectLockMode::Governance),
        "COMPLIANCE" => Ok(ObjectLockMode::Compliance),
        _ => Err(StorageError::Io(format!(
            "invalid S3 retention mode '{mode}': expected GOVERNANCE or COMPLIANCE"
        ))),
    }
}

/// Parse the value of the S3 setting `name` (`None` = not set).
///
/// A value that is not valid UTF-8 is an error rather than counting as unset,
/// which would quietly select local storage or a default instead.
pub fn parse_setting(name: &str, value: Option<OsString>) -> Result<Option<String>, String> {
    value
        .map(|value| {
            value.into_string().map_err(|value| {
                format!(
                    "{name} has an invalid value {:?}: not valid UTF-8",
                    value.to_string_lossy()
                )
            })
        })
        .transpose()
}

/// Retention period used when `S3_RETENTION_YEARS` is unset or empty.
const DEFAULT_RETENTION_YEARS: u32 = 7;

/// Parse the value of `S3_RETENTION_YEARS` (`None` = not set).
///
/// Anything but a whole number above 0 is an error, so that a typo such as
/// `10y` does not quietly lock objects for the default period instead, and 0
/// would give every object a retain-until date that has already passed.
pub fn parse_retention_years(value: Option<&OsStr>) -> Result<u32, String> {
    let value = value.unwrap_or_default();
    if value.is_empty() {
        return Ok(DEFAULT_RETENTION_YEARS);
    }
    let invalid = || {
        format!(
            "S3_RETENTION_YEARS has an invalid value {:?}: expected a whole number of years above 0",
            value.to_string_lossy()
        )
    };
    let text = value.to_str().ok_or_else(invalid)?;
    // `u32::from_str` also takes a leading '+'.
    if !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    match text.parse() {
        Ok(0) | Err(_) => Err(invalid()),
        Ok(years) => Ok(years),
    }
}

/// Compute the retention expiry `years` calendar years after `now`.
fn retain_until(now: DateTime<Utc>, years: u32) -> Result<DateTime<Utc>, StorageError> {
    years
        .checked_mul(12)
        .and_then(|months| now.checked_add_months(Months::new(months)))
        .ok_or_else(|| {
            StorageError::Io(format!(
                "S3 retention of {years} years is out of the supported date range"
            ))
        })
}

/// Build the full S3 key for a proof ID stored at `now`.
fn s3_key(prefix: &str, now: DateTime<Utc>, id: &str) -> String {
    format!("{prefix}{}/{id}.json", now.format("%Y-%m-%d"))
}

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
    /// - `years` — Retention period in calendar years.
    /// - `prefix` — Key prefix (e.g. `"proofs/"`).
    ///
    /// # Errors
    /// Returns an error for any other retention mode, and for a retention
    /// period that does not end at a representable date.
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

        Self::with_client(Client::new(&config), bucket, mode, years, prefix)
    }

    /// Create the backend on an existing S3 client.
    fn with_client(
        client: Client,
        bucket: &str,
        mode: &str,
        years: u32,
        prefix: &str,
    ) -> Result<Self, StorageError> {
        let retention_mode = parse_retention_mode(mode)?;
        retain_until(Utc::now(), years)?;

        Ok(Self {
            client,
            bucket: bucket.to_string(),
            prefix: prefix.to_string(),
            retention_mode,
            retention_years: years,
        })
    }

    /// The Object Lock retention mode in effect.
    pub fn retention_mode(&self) -> &str {
        self.retention_mode.as_str()
    }

    /// List every object under the prefix, page by page (S3 returns at most
    /// 1000 per request), until `visit` returns `false` or the listing ends.
    async fn for_each_object(
        &self,
        mut visit: impl FnMut(&Object) -> bool + Send,
    ) -> Result<(), StorageError> {
        let mut pages = self
            .client
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(&self.prefix)
            .into_paginator()
            .send();

        while let Some(page) = pages.next().await {
            let page =
                page.map_err(|e| StorageError::Io(format!("S3 ListObjectsV2 failed: {e}")))?;
            if !page.contents().iter().all(&mut visit) {
                break;
            }
        }
        Ok(())
    }

    /// Whether an object with exactly `key` exists. One list request is
    /// enough: `key` sorts before every other key that starts with it, so it
    /// is on the first page if it exists.
    async fn key_exists(&self, key: &str) -> Result<bool, StorageError> {
        let page = self
            .client
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(key)
            .send()
            .await
            .map_err(|e| StorageError::Io(format!("S3 ListObjectsV2 failed: {e}")))?;
        Ok(page.contents().iter().any(|obj| obj.key() == Some(key)))
    }

    /// Find the key of a proof, which could be under any date partition.
    async fn find_key(&self, id: &str) -> Result<Option<String>, StorageError> {
        let suffix = format!("{id}.json");
        let mut found = None;
        self.for_each_object(|obj| {
            found = obj.key().filter(|k| k.ends_with(&suffix)).map(String::from);
            found.is_none()
        })
        .await?;
        Ok(found)
    }
}

#[async_trait]
impl ProofStorage for S3Storage {
    /// Write a new proof under today's date partition.
    ///
    /// Only the key about to be written is checked, not every date partition
    /// (which would list the whole prefix, one request per 1000 objects, on
    /// every store). That is enough because the only caller, `submit_proof`,
    /// makes each ID a fresh UUID v4, so it cannot already exist under an
    /// earlier date; the check guards only against writing the same key twice.
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

        let now = Utc::now();
        let key = s3_key(&self.prefix, now, id);
        let retain_until = retain_until(now, self.retention_years)?;

        if self.key_exists(&key).await? {
            return Err(StorageError::AlreadyExists(id.to_string()));
        }

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(ByteStream::from(data.to_vec()))
            .content_type("application/json")
            .object_lock_mode(self.retention_mode.clone())
            .object_lock_retain_until_date(aws_sdk_s3::primitives::DateTime::from_millis(
                retain_until.timestamp_millis(),
            ))
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

        let key = self
            .find_key(id)
            .await?
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

        Ok(self.find_key(id).await?.is_some())
    }

    async fn list(&self, offset: usize, limit: usize) -> Result<Vec<String>, StorageError> {
        let mut skipped = 0;
        let mut ids = Vec::new();
        self.for_each_object(|obj| {
            // Extract proof ID from key like "proofs/2024-01-15/abc.json"
            let id = obj
                .key()
                .and_then(|key| key.rsplit('/').next())
                .and_then(|filename| filename.strip_suffix(".json"));
            if let Some(id) = id {
                if skipped < offset {
                    skipped += 1;
                } else if ids.len() < limit {
                    ids.push(id.to_string());
                }
            }
            ids.len() < limit
        })
        .await?;

        Ok(ids)
    }

    fn storage_type(&self) -> &str {
        "s3"
    }
}

// ---------------------------------------------------------------------------
// Tests (against a stub S3 endpoint on the loopback interface)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::{BTreeMap, HashMap};
    use std::sync::{Arc, Mutex};

    use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
    use axum::body::Bytes;
    use axum::extract::{Query, State};
    use axum::http::{HeaderMap, Method, StatusCode, Uri};

    /// Largest page S3 returns from ListObjectsV2.
    const PAGE: usize = 1000;
    const BUCKET: &str = "test-bucket";

    /// Contents and request log of the stub S3 endpoint.
    #[derive(Default)]
    struct Stub {
        objects: BTreeMap<String, Vec<u8>>,
        /// Continuation token of each ListObjectsV2 request.
        lists: Vec<Option<String>>,
        /// Key and headers of each PutObject request.
        puts: Vec<(String, HeaderMap)>,
    }

    type SharedStub = Arc<Mutex<Stub>>;

    /// ListObjectsV2 (keys in order, pages of 1000), GetObject and PutObject.
    async fn stub_s3(
        State(stub): State<SharedStub>,
        method: Method,
        uri: Uri,
        Query(query): Query<HashMap<String, String>>,
        headers: HeaderMap,
        body: Bytes,
    ) -> (StatusCode, Vec<u8>) {
        let mut stub = stub.lock().unwrap();
        let path = uri.path().trim_start_matches('/');
        let key = path
            .strip_prefix(BUCKET)
            .expect("request for another bucket")
            .trim_start_matches('/')
            .to_string();

        if method == Method::GET && key.is_empty() {
            assert_eq!(query.get("list-type").map(String::as_str), Some("2"));
            let prefix = query.get("prefix").cloned().unwrap_or_default();
            let token = query.get("continuation-token").cloned();
            stub.lists.push(token.clone());

            // The token is the last key of the previous page.
            let rest: Vec<&String> = stub
                .objects
                .keys()
                .filter(|k| k.starts_with(&prefix))
                .filter(|k| token.as_ref().is_none_or(|t| *k > t))
                .collect();
            let page = &rest[..rest.len().min(PAGE)];
            let truncated = rest.len() > PAGE;
            let contents: String = page
                .iter()
                .map(|k| format!("<Contents><Key>{k}</Key></Contents>"))
                .collect();
            let next = if truncated {
                format!(
                    "<NextContinuationToken>{}</NextContinuationToken>",
                    page[PAGE - 1]
                )
            } else {
                String::new()
            };
            let xml = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
                 <ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                 <IsTruncated>{truncated}</IsTruncated>{next}{contents}\
                 </ListBucketResult>"
            );
            (StatusCode::OK, xml.into_bytes())
        } else if method == Method::GET {
            match stub.objects.get(&key) {
                Some(data) => (StatusCode::OK, data.clone()),
                None => (StatusCode::NOT_FOUND, Vec::new()),
            }
        } else if method == Method::PUT {
            stub.objects.insert(key.clone(), body.to_vec());
            stub.puts.push((key, headers));
            (StatusCode::OK, Vec::new())
        } else {
            (StatusCode::METHOD_NOT_ALLOWED, Vec::new())
        }
    }

    /// A client for `endpoint` with static credentials.
    ///
    /// Below behavior version v2026_01_12 the SDK uses its older HTTP client,
    /// which has no proxy support. From v2026_01_12 on its client sends
    /// requests through the proxy in `HTTP_PROXY`, which does not exempt the
    /// loopback stub, so the tests would fail on a machine with a proxy set.
    /// The tests therefore pin an older version, and so run the older HTTP
    /// client; the service itself keeps the latest.
    fn make_client(endpoint: &str) -> Client {
        #[allow(deprecated)]
        let behavior_version = BehaviorVersion::v2025_01_17();
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(behavior_version)
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new("test", "test", None, None, "test"))
            .endpoint_url(endpoint)
            .force_path_style(true)
            .build();
        Client::from_conf(config)
    }

    /// Start the stub on a free port and return a backend that uses it.
    async fn make_storage(objects: BTreeMap<String, Vec<u8>>) -> (S3Storage, SharedStub) {
        let stub = Arc::new(Mutex::new(Stub {
            objects,
            ..Stub::default()
        }));
        let app = axum::Router::new()
            .fallback(stub_s3)
            .with_state(stub.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });

        let storage =
            S3Storage::with_client(make_client(&endpoint), BUCKET, "COMPLIANCE", 7, "proofs/")
                .unwrap();
        (storage, stub)
    }

    /// `count` objects whose IDs are `proof-00000`, `proof-00001`, ...
    fn make_objects(count: usize) -> BTreeMap<String, Vec<u8>> {
        (0..count)
            .map(|i| {
                (
                    format!("proofs/2026-01-01/proof-{i:05}.json"),
                    format!("data {i}").into_bytes(),
                )
            })
            .collect()
    }

    fn list_count(stub: &SharedStub) -> usize {
        stub.lock().unwrap().lists.len()
    }

    #[tokio::test]
    async fn test_get_and_exists_search_every_page() {
        let (storage, stub) = make_storage(make_objects(2 * PAGE + 1)).await;

        // First and last key of each page, and the page each is found on.
        for (i, pages) in [(0, 1), (999, 1), (1000, 2), (1999, 2), (2000, 3)] {
            let id = format!("proof-{i:05}");

            let before = list_count(&stub);
            let data = storage.get(&id).await.unwrap();
            assert_eq!(data, format!("data {i}").into_bytes());
            assert_eq!(list_count(&stub) - before, pages, "get {id}");

            let before = list_count(&stub);
            assert!(storage.exists(&id).await.unwrap(), "exists {id}");
            assert_eq!(list_count(&stub) - before, pages, "exists {id}");
        }

        let before = list_count(&stub);
        assert!(!storage.exists("missing").await.unwrap());
        assert!(matches!(
            storage.get("missing").await,
            Err(StorageError::NotFound(_))
        ));
        assert_eq!(list_count(&stub) - before, 6);
    }

    #[tokio::test]
    async fn test_listing_ends_on_a_full_page() {
        let (storage, stub) = make_storage(make_objects(PAGE)).await;

        assert!(!storage.exists("missing").await.unwrap());
        assert_eq!(stub.lock().unwrap().lists, vec![None]);
    }

    #[tokio::test]
    async fn test_list_spans_pages() {
        let (storage, _stub) = make_storage(make_objects(2 * PAGE + 1)).await;
        let ids = |range: std::ops::Range<usize>| -> Vec<String> {
            range.map(|i| format!("proof-{i:05}")).collect()
        };

        assert_eq!(storage.list(0, 3).await.unwrap(), ids(0..3));
        assert_eq!(storage.list(998, 4).await.unwrap(), ids(998..1002));
        assert_eq!(storage.list(1500, 10).await.unwrap(), ids(1500..1510));
        assert_eq!(storage.list(1990, 100).await.unwrap(), ids(1990..2001));
        assert_eq!(storage.list(0, usize::MAX).await.unwrap(), ids(0..2001));
        assert!(storage.list(2001, 10).await.unwrap().is_empty());
        assert!(storage.list(0, 0).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_store_rejects_a_key_that_already_exists() {
        // The key under today's date, and under tomorrow's in case the date
        // changes while the test runs.
        let today = Utc::now();
        let keys = [today, today + chrono::Days::new(1)].map(|t| s3_key("proofs/", t, "taken"));
        let mut objects = make_objects(2 * PAGE + 1);
        for key in &keys {
            objects.insert(key.clone(), b"first".to_vec());
        }
        let (storage, stub) = make_storage(objects).await;

        let result = storage.store("taken", b"second").await;

        assert!(matches!(result, Err(StorageError::AlreadyExists(_))));
        assert!(stub.lock().unwrap().puts.is_empty());
        assert_eq!(list_count(&stub), 1);
        assert_eq!(storage.get("taken").await.unwrap(), b"first");
    }

    #[tokio::test]
    async fn test_store_ignores_other_keys_that_start_with_its_key() {
        let today = Utc::now();
        let mut objects = BTreeMap::new();
        for t in [today, today + chrono::Days::new(1)] {
            let key = s3_key("proofs/", t, "new-proof");
            objects.insert(format!("{key}.old"), b"other".to_vec());
        }
        let (storage, stub) = make_storage(objects).await;

        let key = storage.store("new-proof", b"new data").await.unwrap();

        assert_eq!(stub.lock().unwrap().objects[&key], b"new data");
    }

    #[tokio::test]
    async fn test_store_sends_one_list_request_on_a_large_bucket() {
        let (storage, stub) = make_storage(make_objects(2 * PAGE + 1)).await;

        let key = storage.store("new-proof", b"new data").await.unwrap();

        let stub = stub.lock().unwrap();
        assert_eq!(stub.lists, vec![None]);
        assert_eq!(stub.puts.len(), 1);
        assert_eq!(stub.puts[0].0, key);
    }

    #[tokio::test]
    async fn test_store_beyond_the_first_page_and_read_back() {
        let (storage, stub) = make_storage(make_objects(PAGE)).await;

        let before = Utc::now();
        let key = storage.store("new-proof", b"new data").await.unwrap();
        let after = Utc::now();

        assert!(
            key == s3_key("proofs/", before, "new-proof")
                || key == s3_key("proofs/", after, "new-proof"),
            "{key}"
        );
        assert_eq!(storage.get("new-proof").await.unwrap(), b"new data");
        assert_eq!(list_count(&stub), 3);

        let stub = stub.lock().unwrap();
        let (put_key, headers) = &stub.puts[0];
        assert_eq!(put_key, &key);
        assert_eq!(headers["x-amz-object-lock-mode"], "COMPLIANCE");
        // The header has millisecond precision.
        let retain_until_ms = DateTime::parse_from_rfc3339(
            headers["x-amz-object-lock-retain-until-date"]
                .to_str()
                .unwrap(),
        )
        .unwrap()
        .timestamp_millis();
        let earliest = retain_until(before, 7).unwrap().timestamp_millis();
        let latest = retain_until(after, 7).unwrap().timestamp_millis();
        assert!((earliest..=latest).contains(&retain_until_ms));
    }

    #[test]
    fn test_s3_key_format() {
        let now = utc("2026-03-04T23:59:59Z");
        assert_eq!(
            s3_key("proofs/", now, "test-proof-123"),
            "proofs/2026-03-04/test-proof-123.json"
        );
        assert_eq!(s3_key("", now, "a"), "2026-03-04/a.json");
    }

    #[test]
    fn test_retention_mode_parsing() {
        for mode in ["GOVERNANCE", "governance", "Governance"] {
            assert_eq!(
                parse_retention_mode(mode).unwrap(),
                ObjectLockMode::Governance
            );
        }
        for mode in ["COMPLIANCE", "compliance"] {
            assert_eq!(
                parse_retention_mode(mode).unwrap(),
                ObjectLockMode::Compliance
            );
        }
        for mode in ["COMPLIENCE", "", " COMPLIANCE", "none"] {
            let error = parse_retention_mode(mode).unwrap_err().to_string();
            assert!(
                error.contains(&format!("invalid S3 retention mode '{mode}'")),
                "{error}"
            );
        }
    }

    #[test]
    fn test_construction_checks_the_retention_settings() {
        let build = |mode, years| {
            S3Storage::with_client(
                make_client("http://127.0.0.1:1"),
                BUCKET,
                mode,
                years,
                "proofs/",
            )
        };

        assert_eq!(
            build("compliance", 7).unwrap().retention_mode(),
            "COMPLIANCE"
        );
        assert_eq!(
            build("governance", 7).unwrap().retention_mode(),
            "GOVERNANCE"
        );
        assert!(build("COMPLIENCE", 7).is_err());
        assert!(build("GOVERNANCE", u32::MAX).is_err());
    }

    #[test]
    fn test_retention_uses_calendar_years() {
        let cases = [
            ("2026-11-24T09:26:41Z", 7, "2033-11-24T09:26:41Z"),
            ("2026-11-24T09:26:41Z", 100, "2126-11-24T09:26:41Z"),
            ("2027-12-31T23:59:59Z", 1, "2028-12-31T23:59:59Z"),
            ("2028-02-29T12:00:00Z", 1, "2029-02-28T12:00:00Z"),
            ("2028-02-29T12:00:00Z", 4, "2032-02-29T12:00:00Z"),
        ];
        for (now, years, expected) in cases {
            assert_eq!(
                retain_until(utc(now), years).unwrap(),
                utc(expected),
                "{now} + {years} years"
            );
        }

        let now = utc("2026-11-24T09:26:41Z");
        assert!(retain_until(now, 300_000).is_err());
        assert!(retain_until(now, u32::MAX).is_err());
    }

    #[test]
    fn test_retention_years_parsing() {
        for (value, years) in [
            (None, 7),
            (Some(""), 7),
            (Some("1"), 1),
            (Some("10"), 10),
            (Some("007"), 7),
            (Some("4294967295"), u32::MAX),
        ] {
            assert_eq!(
                parse_retention_years(value.map(OsStr::new)),
                Ok(years),
                "{value:?}"
            );
        }
        for value in [
            "0",
            "00",
            "10 ",
            " 10",
            "10y",
            "+10",
            "-1",
            "1.5",
            "4294967296",
            "seven",
        ] {
            let error = parse_retention_years(Some(OsStr::new(value))).unwrap_err();
            assert_eq!(
                error,
                format!(
                    "S3_RETENTION_YEARS has an invalid value {value:?}: \
                     expected a whole number of years above 0"
                )
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_non_utf8_retention_years_is_an_error() {
        use std::os::unix::ffi::OsStrExt;
        let error = parse_retention_years(Some(OsStr::from_bytes(b"7\xff"))).unwrap_err();
        assert!(
            error.starts_with("S3_RETENTION_YEARS has an invalid value "),
            "{error}"
        );
    }

    #[test]
    fn test_setting_parsing() {
        assert_eq!(parse_setting("S3_REGION", None), Ok(None));
        assert_eq!(
            parse_setting("S3_REGION", Some("".into())),
            Ok(Some(String::new()))
        );
        assert_eq!(
            parse_setting("S3_REGION", Some("eu-west-1".into())),
            Ok(Some("eu-west-1".to_string()))
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_non_utf8_setting_is_an_error() {
        use std::os::unix::ffi::OsStringExt;
        let value = OsString::from_vec(b"COMPLIANCE\xff".to_vec());
        let error = parse_setting("S3_RETENTION_MODE", Some(value)).unwrap_err();
        assert_eq!(
            error,
            "S3_RETENTION_MODE has an invalid value \"COMPLIANCE\u{FFFD}\": not valid UTF-8"
        );
    }

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().to_utc()
    }
}
