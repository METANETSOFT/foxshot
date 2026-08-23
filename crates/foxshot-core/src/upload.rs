//! Upload targets and the upload queue.
//!
//! Two kinds of destination live here:
//!
//! - [`S3Target`] — an S3-compatible object store (Amazon S3 or Cloudflare
//!   R2), authenticated with AWS Signature Version 4. The signing pipeline is
//!   split into its published steps — [`canonical_request`],
//!   [`string_to_sign`], [`signing_key`], [`signature`] and
//!   [`authorization_header`] — so each step is testable on its own against
//!   AWS's documented worked example (see the `sigv4_*` tests below).
//! - [`FreeHostTarget`] — an anonymous host that takes a plain unsigned PUT
//!   and reports the file's URL in its response body.
//!
//! Core performs **no networking**: a target builds and signs its request
//! and hands it to the caller-provided [`Fetch`], which is where all I/O
//! lives. The only ambient input used here is the wall clock (signatures are
//! time-scoped); everything else is data in, data out.

use crate::error::{Error, Result};
use crate::platform::Fetch;
use core::fmt;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

/// HMAC-SHA256, the only MAC SigV4 uses.
type HmacSha256 = Hmac<Sha256>;

/// The algorithm label SigV4 puts into the string to sign and the
/// `Authorization` header.
const ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// The service name both Amazon S3 and Cloudflare R2 sign under.
const SERVICE: &str = "s3";

/// Access credentials for an S3-compatible object store.
///
/// `Debug` is implemented by hand so the secret can never leak into logs:
/// it always prints as `[redacted]`.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    /// Access key id — safe to display.
    pub access_key_id: String,
    /// Secret access key — never displayed, never logged.
    pub secret_access_key: String,
}

impl Credentials {
    /// Creates credentials from the two raw strings (typically read from the
    /// process environment by the caller).
    pub fn new(
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
    ) -> Credentials {
        Credentials {
            access_key_id: access_key_id.into(),
            secret_access_key: secret_access_key.into(),
        }
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"[redacted]")
            .finish()
    }
}

/// Lowercase hex of the SHA-256 of `bytes`.
pub fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex_encode(&digest)
}

/// Lowercase hex encoding (SigV4 hex-digests everything it hashes).
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// HMAC-SHA256 of `data` under `key`.
fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac =
        <HmacSha256 as Mac>::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(data);
    let bytes = mac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    out
}

/// Header canonicalisation shared by [`canonical_request`] and the upload
/// path: names lowercased, values trimmed, pairs sorted by name. Returns the
/// canonical headers block (each `name:value\n`) and the `SignedHeaders`
/// list (`name;name;...`).
fn canonicalize_headers(headers: &[(String, String)]) -> (String, String) {
    let mut sorted: Vec<(String, String)> = headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    sorted.sort();
    let mut canonical = String::new();
    let mut signed = String::new();
    for (name, value) in &sorted {
        if !signed.is_empty() {
            signed.push(';');
        }
        canonical.push_str(name);
        canonical.push(':');
        canonical.push_str(value);
        canonical.push('\n');
        signed.push_str(name);
    }
    (canonical, signed)
}

/// The `SignedHeaders` list for `headers`: lowercased, sorted, joined by
/// `;`. Must agree with the list [`canonical_request`] builds for the same
/// headers — both come from [`canonicalize_headers`].
fn signed_header_names(headers: &[(String, String)]) -> String {
    canonicalize_headers(headers).1
}

/// SigV4 step 1: the canonical request.
///
/// `headers` are the headers to sign as `(name, value)` pairs in any order
/// and case — they are lowercased, trimmed and sorted here, per the
/// specification. `query` must already be a canonical query string (empty
/// for requests without parameters, which is every upload this module
/// makes). `payload_hash` is the lowercase hex SHA-256 of the exact request
/// body — this module never uses `UNSIGNED-PAYLOAD`.
pub fn canonical_request(
    method: &str,
    path: &str,
    query: &str,
    headers: &[(String, String)],
    payload_hash: &str,
) -> String {
    let (canonical_headers, signed_headers) = canonicalize_headers(headers);
    format!("{method}\n{path}\n{query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}")
}

/// The credential scope: `date/region/service/aws4_request`.
pub fn credential_scope(date: &str, region: &str, service: &str) -> String {
    format!("{date}/{region}/{service}/aws4_request")
}

/// SigV4 step 2: the string to sign.
pub fn string_to_sign(amz_date: &str, scope: &str, canonical_request: &str) -> String {
    format!(
        "{ALGORITHM}\n{amz_date}\n{scope}\n{}",
        hex_sha256(canonical_request.as_bytes())
    )
}

/// SigV4 step 3 (key derivation): the signing key, scoped to one date,
/// region and service — never the raw secret.
pub fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> [u8; 32] {
    let k_date = hmac_sha256(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

/// SigV4 step 3: the signature — hex of HMAC-SHA256 of the string to sign
/// under the signing key.
pub fn signature(signing_key: &[u8; 32], string_to_sign: &str) -> String {
    hex_encode(&hmac_sha256(signing_key, string_to_sign.as_bytes()))
}

/// The final `Authorization` header value.
pub fn authorization_header(
    access_key_id: &str,
    scope: &str,
    signed_headers: &str,
    signature: &str,
) -> String {
    format!(
        "{ALGORITHM} Credential={access_key_id}/{scope},\
         SignedHeaders={signed_headers},\
         Signature={signature}"
    )
}

/// Percent-encodes every byte outside the RFC 3986 unreserved set, keeping
/// `/` so a key's path structure survives.
fn uri_encode_path(path: &str) -> String {
    use core::fmt::Write as _;
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(byte as char);
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

/// Content type guessed from the key's extension; uploads here are images.
fn content_type_for(key: &str) -> &'static str {
    match key
        .rsplit('.')
        .next()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    }
}

/// Somewhere a finished capture can be sent.
pub trait UploadTarget {
    /// Short lowercase name for logs and CLI output (`"r2"`, `"s3"`,
    /// `"free"`).
    fn name(&self) -> &str;
    /// Checks the target before anything is sent: the configuration is
    /// complete and the endpoint answers. `fetch` is used for the
    /// reachability probe.
    fn validate(&self, fetch: &dyn Fetch) -> Result<()>;
    /// Uploads `bytes` under `key` and returns the URL to hand to the user.
    fn upload(&self, fetch: &dyn Fetch, bytes: &[u8], key: &str) -> Result<String>;
}

/// An S3-compatible object store as an upload target: Amazon S3 or
/// Cloudflare R2, both signed with AWS Signature Version 4.
#[derive(Debug, Clone)]
pub struct S3Target {
    /// Base endpoint URL — R2: `https://<account_id>.r2.cloudflarestorage.com`,
    /// AWS: `https://<bucket>.s3.<region>.amazonaws.com`.
    pub endpoint: String,
    /// Signing region (`auto` for R2).
    pub region: String,
    /// Bucket name.
    pub bucket: String,
    /// Optional key prefix (a "folder") prepended to every object key.
    pub prefix: Option<String>,
    /// Optional public URL base; when set, [`UploadTarget::upload`] returns
    /// `public_base` joined with the key instead of the endpoint URL.
    pub public_base: Option<String>,
    /// Signing credentials (their `Debug` redacts the secret).
    pub creds: Credentials,
    /// CLI-facing label: `"r2"` or `"s3"`. Set by the constructors.
    label: &'static str,
    /// Whether `endpoint` already names the bucket in its host
    /// (virtual-hosted style, AWS) or the bucket goes into the request path
    /// (R2). Set by the constructors.
    bucket_in_host: bool,
}

impl S3Target {
    /// A Cloudflare R2 target: endpoint
    /// `https://<account_id>.r2.cloudflarestorage.com`, region `auto`,
    /// bucket addressed path-style.
    pub fn r2(account_id: &str, bucket: &str, creds: Credentials) -> S3Target {
        S3Target {
            endpoint: format!("https://{account_id}.r2.cloudflarestorage.com"),
            region: "auto".to_string(),
            bucket: bucket.to_string(),
            prefix: None,
            public_base: None,
            creds,
            label: "r2",
            bucket_in_host: false,
        }
    }

    /// An Amazon S3 target: endpoint
    /// `https://<bucket>.s3.<region>.amazonaws.com` (virtual-hosted style).
    pub fn aws(region: &str, bucket: &str, creds: Credentials) -> S3Target {
        S3Target {
            endpoint: format!("https://{bucket}.s3.{region}.amazonaws.com"),
            region: region.to_string(),
            bucket: bucket.to_string(),
            prefix: None,
            public_base: None,
            creds,
            label: "s3",
            bucket_in_host: true,
        }
    }

    /// Sets the key prefix prepended to every uploaded object.
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> S3Target {
        self.prefix = Some(prefix.into());
        self
    }

    /// Sets the public URL base uploads report back.
    pub fn with_public_base(mut self, base: impl Into<String>) -> S3Target {
        self.public_base = Some(base.into());
        self
    }

    /// The object key for `name` under this target's prefix — always
    /// exactly one slash between prefix and name.
    pub fn object_key(&self, name: &str) -> String {
        let name = name.trim_start_matches('/');
        match self
            .prefix
            .as_deref()
            .map(|prefix| prefix.trim_matches('/'))
        {
            Some(prefix) if !prefix.is_empty() => format!("{prefix}/{name}"),
            _ => name.to_string(),
        }
    }

    /// The URI-encoded request path, including the bucket for path-style
    /// (R2) endpoints.
    fn request_path(&self, key: &str) -> String {
        if self.bucket_in_host {
            format!("/{}", uri_encode_path(key))
        } else {
            format!("/{}/{}", self.bucket, uri_encode_path(key))
        }
    }

    /// The URL the PUT goes to.
    fn request_url(&self, key: &str) -> String {
        format!(
            "{}{}",
            self.endpoint.trim_end_matches('/'),
            self.request_path(key)
        )
    }

    /// The URL handed back to the caller: `public_base` joined with the key
    /// when set, otherwise the endpoint URL.
    fn public_url(&self, key: &str) -> String {
        match &self.public_base {
            Some(base) => format!("{}/{}", base.trim_end_matches('/'), uri_encode_path(key)),
            None => self.request_url(key),
        }
    }

    /// The host part of the endpoint, for the signed `host` header.
    fn host(&self) -> &str {
        self.endpoint
            .split("://")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .filter(|host| !host.is_empty())
            .unwrap_or(&self.endpoint)
    }
}

impl UploadTarget for S3Target {
    fn name(&self) -> &str {
        self.label
    }

    /// Structural checks, then a reachability probe: any HTTP response —
    /// even a 4xx — proves the endpoint answers (an anonymous GET of a
    /// private bucket is *meant* to be refused); only a connection-level
    /// failure invalidates the target.
    fn validate(&self, fetch: &dyn Fetch) -> Result<()> {
        for (what, value) in [
            ("endpoint", self.endpoint.as_str()),
            ("region", self.region.as_str()),
            ("bucket", self.bucket.as_str()),
            ("access key id", self.creds.access_key_id.as_str()),
            ("secret access key", self.creds.secret_access_key.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(Error::Manifest {
                    message: format!("{} target has an empty {what}", self.label),
                });
            }
        }
        match fetch.get(&self.endpoint) {
            Ok(_) => Ok(()),
            Err(Error::Transport { message }) if message.contains("status") => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Builds the PUT for `key`, signs it with SigV4 over the actual body
    /// hash, hands it to `fetch`, and returns the public URL.
    fn upload(&self, fetch: &dyn Fetch, bytes: &[u8], key: &str) -> Result<String> {
        let key = self.object_key(key);
        let url = self.request_url(&key);
        let payload_hash = hex_sha256(bytes);
        let (amz_date, date) = now_amz_dates();
        let headers: Vec<(String, String)> = vec![
            ("host".to_string(), self.host().to_string()),
            ("x-amz-content-sha256".to_string(), payload_hash.clone()),
            ("x-amz-date".to_string(), amz_date.clone()),
        ];
        let scope = credential_scope(&date, &self.region, SERVICE);
        let canonical =
            canonical_request("PUT", &self.request_path(&key), "", &headers, &payload_hash);
        let to_sign = string_to_sign(&amz_date, &scope, &canonical);
        let signing = signing_key(&self.creds.secret_access_key, &date, &self.region, SERVICE);
        let signed = signature(&signing, &to_sign);
        let authorization = authorization_header(
            &self.creds.access_key_id,
            &scope,
            &signed_header_names(&headers),
            &signed,
        );
        let extra: Vec<(String, String)> = vec![
            ("x-amz-date".to_string(), amz_date),
            ("x-amz-content-sha256".to_string(), payload_hash),
            ("authorization".to_string(), authorization),
        ];
        fetch.put_with_headers(&url, bytes, content_type_for(&key), &extra)?;
        Ok(self.public_url(&key))
    }
}

/// An anonymous file host that accepts a plain, unsigned PUT and reports
/// the URL of the uploaded file in its response body (transfer.sh style).
#[derive(Debug, Clone)]
pub struct FreeHostTarget {
    /// Base URL of the host; the object key is appended as the path.
    pub endpoint: String,
}

impl FreeHostTarget {
    /// A free-host target rooted at `endpoint` (no trailing slash needed).
    pub fn new(endpoint: impl Into<String>) -> FreeHostTarget {
        FreeHostTarget {
            endpoint: endpoint.into(),
        }
    }
}

impl UploadTarget for FreeHostTarget {
    fn name(&self) -> &str {
        "free"
    }

    fn validate(&self, fetch: &dyn Fetch) -> Result<()> {
        if self.endpoint.trim().is_empty() {
            return Err(Error::Manifest {
                message: "free target has an empty endpoint".to_string(),
            });
        }
        match fetch.get(&self.endpoint) {
            Ok(_) => Ok(()),
            Err(Error::Transport { message }) if message.contains("status") => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// A plain PUT with no signing. The host is expected to answer with the
    /// file's URL as its response body; anything else is a transport
    /// failure naming exactly that — a guess would be a lie.
    fn upload(&self, fetch: &dyn Fetch, bytes: &[u8], key: &str) -> Result<String> {
        let url = format!(
            "{}/{}",
            self.endpoint.trim_end_matches('/'),
            uri_encode_path(key.trim_start_matches('/'))
        );
        let body = fetch.put(&url, bytes, content_type_for(key))?;
        let reported = body.trim();
        if reported.starts_with("http://") || reported.starts_with("https://") {
            Ok(reported.to_string())
        } else {
            Err(Error::Transport {
                message: format!(
                    "free host {} did not report a URL in its response",
                    self.endpoint
                ),
            })
        }
    }
}

/// One queued upload, with the error of its last failed attempt attached.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingUpload {
    /// Object key the bytes will be stored under.
    pub key: String,
    /// The bytes to upload.
    pub bytes: Vec<u8>,
    /// Error of the most recent failed `drain`, if any — kept so the caller
    /// can see why the item is still pending and retry it.
    pub error: Option<Error>,
}

/// What one [`UploadQueue::drain`] pass achieved.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct DrainReport {
    /// `(key, URL)` of every upload that succeeded, in queue order.
    pub uploaded: Vec<(String, String)>,
}

/// A queue of pending uploads against one target.
///
/// [`UploadQueue::drain`] uploads every pending item; successes leave the
/// queue, failures **stay in it with their error attached** — nothing is
/// ever dropped silently, and the next `drain` retries what failed.
pub struct UploadQueue {
    target: Box<dyn UploadTarget>,
    pending: Vec<PendingUpload>,
}

impl fmt::Debug for UploadQueue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UploadQueue")
            .field("target", &self.target.name())
            .field("pending", &self.pending)
            .finish()
    }
}

impl UploadQueue {
    /// An empty queue uploading to `target`.
    pub fn new(target: Box<dyn UploadTarget>) -> UploadQueue {
        UploadQueue {
            target,
            pending: Vec::new(),
        }
    }

    /// Adds `bytes` to the queue under `key`.
    pub fn push(&mut self, key: impl Into<String>, bytes: Vec<u8>) {
        self.pending.push(PendingUpload {
            key: key.into(),
            bytes,
            error: None,
        });
    }

    /// The items still waiting — including failed ones, with their errors.
    pub fn pending(&self) -> &[PendingUpload] {
        &self.pending
    }

    /// Uploads every pending item through the target. Successes are
    /// reported and leave the queue; failures stay queued with the error
    /// attached to the item.
    pub fn drain(&mut self, fetch: &dyn Fetch) -> DrainReport {
        let mut report = DrainReport::default();
        let mut still_pending = Vec::with_capacity(self.pending.len());
        for mut item in self.pending.drain(..) {
            match self.target.upload(fetch, &item.bytes, &item.key) {
                Ok(url) => report.uploaded.push((item.key, url)),
                Err(error) => {
                    item.error = Some(error);
                    still_pending.push(item);
                }
            }
        }
        self.pending = still_pending;
        report
    }
}

/// Current UTC time as `(amz_date, date)`: `YYYYMMDDTHHMMSSZ` and
/// `YYYYMMDD`, the two formats SigV4 needs. Derived from the wall clock by
/// pure calendar arithmetic — no formatting library, no I/O beyond reading
/// the time.
fn now_amz_dates() -> (String, String) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = (secs / 86_400) as i64;
    let secs_of_day = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    (
        format!(
            "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
            secs_of_day / 3600,
            (secs_of_day % 3600) / 60,
            secs_of_day % 60
        ),
        format!("{year:04}{month:02}{day:02}"),
    )
}

/// Days since 1970-01-01 → `(year, month, day)`, Howard Hinnant's
/// `civil_from_days` algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // day of era: [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // year of era: [0, 399]
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year: [0, 365]
    let mp = (5 * doy + 2) / 153; // month index from March: [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    // === AWS's published SigV4 worked example ==============================
    //
    // Source: "Example: GET Object" in *Authenticating Requests (AWS
    // Signature Version 4)*, Amazon S3 API Reference
    // (docs.aws.amazon.com/AmazonS3/latest/API/sig-v4-header-based-auth.html,
    // retrieved via the Wayback Machine on 2026-08-23). Every expected
    // string below is copied from that page — external truth, not this
    // code's own output. The keys are AWS's public documentation examples.
    const AWS_ACCESS_KEY: &str = "AKIAIOSFODNN7EXAMPLE";
    const AWS_SECRET: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
    const AWS_AMZ_DATE: &str = "20130524T000000Z";
    const AWS_DATE: &str = "20130524";
    const AWS_SCOPE: &str = "20130524/us-east-1/s3/aws4_request";
    const EMPTY_PAYLOAD_HASH: &str =
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn aws_example_headers() -> Vec<(String, String)> {
        vec![
            (
                "Host".to_string(),
                "examplebucket.s3.amazonaws.com".to_string(),
            ),
            ("Range".to_string(), "bytes=0-9".to_string()),
            (
                "x-amz-content-sha256".to_string(),
                EMPTY_PAYLOAD_HASH.to_string(),
            ),
            ("x-amz-date".to_string(), AWS_AMZ_DATE.to_string()),
        ]
    }

    fn aws_example_canonical_request() -> String {
        canonical_request(
            "GET",
            "/test.txt",
            "",
            &aws_example_headers(),
            EMPTY_PAYLOAD_HASH,
        )
    }

    fn aws_example_string_to_sign() -> String {
        string_to_sign(AWS_AMZ_DATE, AWS_SCOPE, &aws_example_canonical_request())
    }

    #[test]
    fn sigv4_canonical_request_matches_aws_example() {
        let expected = "GET\n\
             /test.txt\n\
             \n\
             host:examplebucket.s3.amazonaws.com\n\
             range:bytes=0-9\n\
             x-amz-content-sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n\
             x-amz-date:20130524T000000Z\n\
             \n\
             host;range;x-amz-content-sha256;x-amz-date\n\
             e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(aws_example_canonical_request(), expected);
    }

    #[test]
    fn sigv4_string_to_sign_matches_aws_example() {
        let expected = "AWS4-HMAC-SHA256\n\
             20130524T000000Z\n\
             20130524/us-east-1/s3/aws4_request\n\
             7344ae5b7ee6c3e7e6b0fe0640412a37625d1fbfff95c48bbb2dc43964946972";
        assert_eq!(aws_example_string_to_sign(), expected);
    }

    #[test]
    fn sigv4_signature_matches_aws_example() {
        let key = signing_key(AWS_SECRET, AWS_DATE, "us-east-1", "s3");
        assert_eq!(
            signature(&key, &aws_example_string_to_sign()),
            "f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    #[test]
    fn sigv4_authorization_header_matches_aws_example() {
        let key = signing_key(AWS_SECRET, AWS_DATE, "us-east-1", "s3");
        let signed = signature(&key, &aws_example_string_to_sign());
        let header = authorization_header(
            AWS_ACCESS_KEY,
            AWS_SCOPE,
            "host;range;x-amz-content-sha256;x-amz-date",
            &signed,
        );
        assert_eq!(
            header,
            "AWS4-HMAC-SHA256 \
             Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request,\
             SignedHeaders=host;range;x-amz-content-sha256;x-amz-date,\
             Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    // === Credentials =======================================================

    #[test]
    fn credentials_debug_redacts_the_secret() {
        let creds = Credentials::new("AKIDEXAMPLE", "super-secret-value");
        let shown = format!("{creds:?}");
        assert!(
            shown.contains("[redacted]"),
            "secret must print redacted: {shown}"
        );
        assert!(
            !shown.contains("super-secret-value"),
            "secret value must never appear in Debug output: {shown}"
        );
        assert!(
            shown.contains("AKIDEXAMPLE"),
            "access key id is safe to show: {shown}"
        );
    }

    #[test]
    fn s3_target_debug_redacts_the_secret() {
        let target = S3Target::aws(
            "us-east-1",
            "bucket",
            Credentials::new("AKIDEXAMPLE", "super-secret-value"),
        );
        let shown = format!("{target:?}");
        assert!(
            !shown.contains("super-secret-value"),
            "target Debug leaks the secret: {shown}"
        );
    }

    // === S3Target construction and keys ====================================

    #[test]
    fn r2_endpoint_is_built_from_account_id() {
        let target = S3Target::r2("abc123", "shots", Credentials::new("id", "secret"));
        assert_eq!(target.endpoint, "https://abc123.r2.cloudflarestorage.com");
        assert_eq!(target.region, "auto");
        assert_eq!(target.name(), "r2");
    }

    #[test]
    fn aws_endpoint_is_virtual_hosted_style() {
        let target = S3Target::aws("eu-central-1", "shots", Credentials::new("id", "secret"));
        assert_eq!(
            target.endpoint,
            "https://shots.s3.eu-central-1.amazonaws.com"
        );
        assert_eq!(target.region, "eu-central-1");
        assert_eq!(target.name(), "s3");
    }

    #[test]
    fn key_with_prefix_joins_without_double_slash() {
        let target = S3Target::r2("abc123", "shots", Credentials::new("id", "secret"));
        assert_eq!(
            target.clone().with_prefix("shots/").object_key("a.png"),
            "shots/a.png"
        );
        assert_eq!(
            target.clone().with_prefix("shots").object_key("a.png"),
            "shots/a.png"
        );
        assert_eq!(
            target.clone().with_prefix("/shots/").object_key("/a.png"),
            "shots/a.png"
        );
        assert_eq!(target.object_key("a.png"), "a.png");
    }

    // === Upload flow over a recording Fetch ================================

    /// A `Fetch` that records every signed PUT and answers like a free host.
    struct RecordingFetch {
        puts: RefCell<Vec<(String, String)>>,
    }

    impl RecordingFetch {
        fn new() -> RecordingFetch {
            RecordingFetch {
                puts: RefCell::new(Vec::new()),
            }
        }
    }

    impl Fetch for RecordingFetch {
        fn get(&self, _url: &str) -> Result<Vec<u8>> {
            Ok(Vec::new())
        }

        fn put(&self, _url: &str, _body: &[u8], _content_type: &str) -> Result<String> {
            Ok("https://free.example/x.png".to_string())
        }

        fn put_with_headers(
            &self,
            url: &str,
            _body: &[u8],
            _content_type: &str,
            headers: &[(String, String)],
        ) -> Result<String> {
            let auth = headers
                .iter()
                .find(|(name, _)| name == "authorization")
                .map(|(_, value)| value.clone())
                .unwrap_or_default();
            self.puts.borrow_mut().push((url.to_string(), auth));
            Ok(String::new())
        }
    }

    /// A `Fetch` that fails at the connection level.
    struct FailingFetch;

    impl Fetch for FailingFetch {
        fn get(&self, _url: &str) -> Result<Vec<u8>> {
            Err(Error::Transport {
                message: "connection refused".to_string(),
            })
        }

        fn put(&self, _url: &str, _body: &[u8], _content_type: &str) -> Result<String> {
            Err(Error::Transport {
                message: "connection refused".to_string(),
            })
        }
    }

    fn example_target() -> S3Target {
        S3Target::r2("abc123", "shots", Credentials::new("AKIDEXAMPLE", "secret"))
            .with_prefix("captures")
    }

    #[test]
    fn s3_upload_signs_and_returns_endpoint_url() {
        let fetch = RecordingFetch::new();
        let url = example_target()
            .upload(&fetch, b"png-bytes", "a.png")
            .unwrap();
        assert_eq!(
            url,
            "https://abc123.r2.cloudflarestorage.com/shots/captures/a.png"
        );
        let puts = fetch.puts.borrow();
        assert_eq!(puts.len(), 1);
        assert_eq!(puts[0].0, url);
        assert!(
            puts[0]
                .1
                .starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/"),
            "signed authorization header expected, got: {}",
            puts[0].1
        );
        assert!(
            puts[0]
                .1
                .contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date")
        );
    }

    #[test]
    fn s3_upload_returns_public_base_url_when_set() {
        let fetch = RecordingFetch::new();
        let target = example_target().with_public_base("https://cdn.example.com/");
        let url = target.upload(&fetch, b"png-bytes", "a.png").unwrap();
        assert_eq!(url, "https://cdn.example.com/captures/a.png");
    }

    #[test]
    fn validate_rejects_empty_configuration() {
        let fetch = RecordingFetch::new();
        let target = S3Target::r2("abc123", "", Credentials::new("AKIDEXAMPLE", "secret"));
        assert!(matches!(
            target.validate(&fetch),
            Err(Error::Manifest { .. })
        ));
    }

    #[test]
    fn validate_fails_when_endpoint_is_unreachable() {
        let target = example_target();
        assert!(matches!(
            target.validate(&FailingFetch),
            Err(Error::Transport { .. })
        ));
    }

    // === FreeHostTarget =====================================================

    #[test]
    fn free_host_returns_the_reported_url() {
        let fetch = RecordingFetch::new();
        let target = FreeHostTarget::new("https://free.example/");
        let url = target.upload(&fetch, b"png-bytes", "x.png").unwrap();
        assert_eq!(url, "https://free.example/x.png");
    }

    #[test]
    fn free_host_without_a_url_in_response_is_a_transport_error() {
        struct SilentFetch;
        impl Fetch for SilentFetch {
            fn get(&self, _url: &str) -> Result<Vec<u8>> {
                Ok(Vec::new())
            }
            fn put(&self, _url: &str, _body: &[u8], _content_type: &str) -> Result<String> {
                Ok("OK".to_string())
            }
        }
        let target = FreeHostTarget::new("https://free.example");
        let error = target
            .upload(&SilentFetch, b"png-bytes", "x.png")
            .unwrap_err();
        match error {
            Error::Transport { message } => {
                assert!(
                    message.contains("did not report a URL"),
                    "unexpected: {message}"
                );
                assert!(
                    message.contains("free.example"),
                    "error must name the host: {message}"
                );
            }
            other => panic!("expected Transport, got {other:?}"),
        }
    }

    // === UploadQueue ========================================================

    #[test]
    fn drain_uploads_everything_and_empties_the_queue() {
        let fetch = RecordingFetch::new();
        let mut queue = UploadQueue::new(Box::new(FreeHostTarget::new("https://free.example")));
        queue.push("a.png", b"a".to_vec());
        queue.push("b.png", b"b".to_vec());
        let report = queue.drain(&fetch);
        assert_eq!(report.uploaded.len(), 2);
        assert!(queue.pending().is_empty());
    }

    #[test]
    fn failed_items_stay_queued_with_their_error_and_retry() {
        let mut queue = UploadQueue::new(Box::new(FreeHostTarget::new("https://free.example")));
        queue.push("a.png", b"a".to_vec());

        let report = queue.drain(&FailingFetch);
        assert!(report.uploaded.is_empty());
        assert_eq!(queue.pending().len(), 1);
        assert!(
            matches!(queue.pending()[0].error, Some(Error::Transport { .. })),
            "failure must stay attached to the item: {:?}",
            queue.pending()[0].error
        );

        // A later drain with working transport retries the same item.
        let report = queue.drain(&RecordingFetch::new());
        assert_eq!(report.uploaded.len(), 1);
        assert!(queue.pending().is_empty());
    }

    // === Clock helpers =======================================================

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2013-05-24, the date of AWS's example, is day 15849.
        assert_eq!(civil_from_days(15_849), (2013, 5, 24));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
    }

    #[test]
    fn uri_encode_path_keeps_unreserved_and_slashes() {
        assert_eq!(
            uri_encode_path("shots/a-b_c.d~e.png"),
            "shots/a-b_c.d~e.png"
        );
        assert_eq!(uri_encode_path("a b.png"), "a%20b.png");
    }
}
