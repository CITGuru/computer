//! A bucket, addressed by key.

use crate::{Blobs, Error, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

pub struct S3 {
    client: reqwest::Client,
    /// No trailing slash.
    endpoint: String,
    bucket: String,
    region: String,
    access_key: String,
    secret_key: String,
    /// Empty, or ending in a slash.
    prefix: String,
}

impl S3 {
    pub fn new(
        endpoint: impl Into<String>,
        bucket: impl Into<String>,
        region: impl Into<String>,
        access_key: impl Into<String>,
        secret_key: impl Into<String>,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|error| Error::Unavailable(format!("no http client: {error}")))?;

        Ok(Self {
            client,
            endpoint: endpoint.into().trim_end_matches('/').to_string(),
            bucket: bucket.into(),
            region: region.into(),
            access_key: access_key.into(),
            secret_key: secret_key.into(),
            prefix: String::new(),
        })
    }

    /// A prefix inside the bucket, so one bucket can hold more than this.
    pub fn under(mut self, prefix: impl Into<String>) -> Self {
        let given = prefix.into();

        self.prefix = match given.is_empty() || given.ends_with('/') {
            true => given,
            false => format!("{given}/"),
        };

        self
    }

    pub fn from_env() -> Result<Self> {
        let need = |name: &str| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| Error::Unavailable(format!("{name} is not set")))
        };

        let store = Self::new(
            need("COMPUTER_S3_ENDPOINT")?,
            need("COMPUTER_S3_BUCKET")?,
            std::env::var("COMPUTER_S3_REGION").unwrap_or_else(|_| "us-east-1".to_string()),
            need("AWS_ACCESS_KEY_ID")?,
            need("AWS_SECRET_ACCESS_KEY")?,
        )?;

        Ok(store.under(std::env::var("COMPUTER_S3_PREFIX").unwrap_or_default()))
    }

    fn at(&self, key: &str) -> String {
        format!("{}{key}", self.prefix)
    }

    /// The path a key lives at, bucket included.
    fn path_of(&self, key: Option<&str>) -> String {
        match key {
            Some(key) => format!("/{}/{}", self.bucket, self.at(key)),
            None => format!("/{}", self.bucket),
        }
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(String, String)],
        body: Vec<u8>,
    ) -> Result<reqwest::Response> {
        let signed = self.sign(method.as_str(), path, query, &body, Utc::now());

        let mut url = format!("{}{}", self.endpoint, encoded_path(path));
        if !query.is_empty() {
            url.push('?');
            url.push_str(&canonical_query(query));
        }

        let mut request = self.client.request(method, &url);
        for (name, value) in signed {
            request = request.header(name, value);
        }

        request
            .body(body)
            .send()
            .await
            .map_err(|error| Error::Unavailable(format!("{url}: {error}")))
    }

    /// The headers that authorise one request.
    fn sign(
        &self,
        method: &str,
        path: &str,
        query: &[(String, String)],
        body: &[u8],
        now: DateTime<Utc>,
    ) -> Vec<(String, String)> {
        let stamp = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date = now.format("%Y%m%d").to_string();
        let host = host_of(&self.endpoint);
        let payload = hex::encode(Sha256::digest(body));

        let canonical = format!(
            "{method}\n{}\n{}\nhost:{host}\nx-amz-content-sha256:{payload}\nx-amz-date:{stamp}\n\n\
             {SIGNED}\n{payload}",
            encoded_path(path),
            canonical_query(query),
        );

        let scope = format!("{date}/{}/s3/aws4_request", self.region);
        let to_sign = format!(
            "AWS4-HMAC-SHA256\n{stamp}\n{scope}\n{}",
            hex::encode(Sha256::digest(canonical.as_bytes()))
        );

        let signature = hex::encode(hmac(
            &signing_key(&self.secret_key, &date, &self.region, "s3"),
            to_sign.as_bytes(),
        ));

        vec![
            ("host".to_string(), host),
            ("x-amz-date".to_string(), stamp),
            ("x-amz-content-sha256".to_string(), payload),
            (
                "authorization".to_string(),
                format!(
                    "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={SIGNED}, \
                     Signature={signature}",
                    self.access_key
                ),
            ),
        ]
    }
}

/// The headers every request here signs, in the order the algorithm wants them.
const SIGNED: &str = "host;x-amz-content-sha256;x-amz-date";

#[async_trait]
impl Blobs for S3 {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let path = self.path_of(Some(key));
        let answer = self
            .send(reqwest::Method::GET, &path, &[], Vec::new())
            .await?;

        if answer.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let answer = refused(key, answer)?;

        answer
            .bytes()
            .await
            .map(|bytes| Some(bytes.to_vec()))
            .map_err(|error| Error::Unavailable(format!("{key}: {error}")))
    }

    async fn put(&self, key: &str, bytes: &[u8]) -> Result<()> {
        let path = self.path_of(Some(key));
        let answer = self
            .send(reqwest::Method::PUT, &path, &[], bytes.to_vec())
            .await?;

        refused(key, answer).map(|_| ())
    }

    async fn list(&self, prefix: &str, start_after: Option<&str>) -> Result<Vec<String>> {
        let path = self.path_of(None);
        let mut keys = Vec::new();
        let mut carry: Option<String> = None;

        loop {
            let mut query = vec![
                ("list-type".to_string(), "2".to_string()),
                ("prefix".to_string(), self.at(prefix)),
            ];
            if let Some(after) = start_after {
                query.push(("start-after".to_string(), self.at(after)));
            }
            if let Some(token) = &carry {
                query.push(("continuation-token".to_string(), token.clone()));
            }

            let answer = self
                .send(reqwest::Method::GET, &path, &query, Vec::new())
                .await?;
            let body = refused(prefix, answer)?
                .text()
                .await
                .map_err(|error| Error::Unavailable(format!("{prefix}: {error}")))?;

            for key in tagged(&body, "Key") {
                match key.strip_prefix(&self.prefix) {
                    Some(theirs) => keys.push(theirs.to_string()),
                    None => continue,
                }
            }

            carry = tagged(&body, "NextContinuationToken").into_iter().next();
            if carry.is_none() {
                break;
            }
        }

        Ok(keys)
    }

    async fn delete_prefix(&self, prefix: &str) -> Result<()> {
        for key in self.list(prefix, None).await? {
            let path = self.path_of(Some(&key));
            let answer = self
                .send(reqwest::Method::DELETE, &path, &[], Vec::new())
                .await?;

            if answer.status() != reqwest::StatusCode::NOT_FOUND {
                refused(&key, answer)?;
            }
        }

        Ok(())
    }
}

fn refused(key: &str, answer: reqwest::Response) -> Result<reqwest::Response> {
    let status = answer.status();

    match status.is_success() {
        true => Ok(answer),
        false => Err(Error::Unavailable(format!(
            "{key}: the bucket said {status}"
        ))),
    }
}

type Sha = Hmac<Sha256>;

fn hmac(key: &[u8], message: &[u8]) -> Vec<u8> {
    let mut mac = <Sha as Mac>::new_from_slice(key).expect("hmac takes a key of any length");
    mac.update(message);
    mac.finalize().into_bytes().to_vec()
}

/// The chain the algorithm derives a per-day, per-region key with.
fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let start = hmac(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let regional = hmac(&start, region.as_bytes());
    let scoped = hmac(&regional, service.as_bytes());

    hmac(&scoped, b"aws4_request")
}

fn canonical_query(query: &[(String, String)]) -> String {
    let mut pairs: Vec<String> = query
        .iter()
        .map(|(name, value)| format!("{}={}", encode(name), encode(value)))
        .collect();
    pairs.sort();

    pairs.join("&")
}

/// A path, with each segment encoded and the separators left alone.
fn encoded_path(path: &str) -> String {
    path.split('/').map(encode).collect::<Vec<_>>().join("/")
}

/// Everything outside the unreserved set, including the slash.
fn encode(part: &str) -> String {
    let mut out = String::with_capacity(part.len());

    for byte in part.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }

    out
}

fn host_of(endpoint: &str) -> String {
    endpoint
        .split_once("://")
        .map_or(endpoint, |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The text inside every `<tag>` in a listing.
fn tagged(body: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut found = Vec::new();

    for after in body.split(&open).skip(1) {
        let Some((inside, _)) = after.split_once(&close) else {
            continue;
        };
        found.push(unescaped(inside));
    }

    found
}

fn unescaped(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conformance;

    #[test]
    fn test_the_signing_key_matches_the_published_one() {
        assert_eq!(
            hex::encode(signing_key(
                "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
                "20150830",
                "us-east-1",
                "iam",
            )),
            "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9"
        );
    }

    #[test]
    fn test_only_the_unreserved_set_survives_encoding() {
        assert_eq!(encode("aZ09-._~"), "aZ09-._~");
        assert_eq!(encode("a/b"), "a%2Fb", "a slash is not unreserved");
        assert_eq!(encode("a b"), "a%20b", "and a space is not a plus");
        assert_eq!(encode("a+b"), "a%2Bb");
    }

    #[test]
    fn test_a_path_keeps_its_separators() {
        assert_eq!(
            encoded_path("/bucket/boxes/box_1/main.json"),
            "/bucket/boxes/box_1/main.json"
        );
        assert_eq!(
            encoded_path("/bucket/boxes/a b/main.json"),
            "/bucket/boxes/a%20b/main.json",
            "each segment is encoded and the slashes are left"
        );
    }

    #[test]
    fn test_a_query_is_sorted_before_it_is_signed() {
        let query = [
            ("prefix".to_string(), "boxes/box_1/".to_string()),
            ("list-type".to_string(), "2".to_string()),
        ];

        assert_eq!(
            canonical_query(&query),
            "list-type=2&prefix=boxes%2Fbox_1%2F"
        );
    }

    #[test]
    fn test_a_listing_gives_back_its_keys_and_its_carry() {
        let body = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult>
  <Name>state</Name>
  <IsTruncated>true</IsTruncated>
  <Contents><Key>boxes/box_1/main.json</Key><Size>12</Size></Contents>
  <Contents><Key>boxes/box_1/a&amp;b.png</Key><Size>3</Size></Contents>
  <NextContinuationToken>1ueGcxLPRx1Tr</NextContinuationToken>
</ListBucketResult>"#;

        assert_eq!(
            tagged(body, "Key"),
            vec![
                "boxes/box_1/main.json".to_string(),
                "boxes/box_1/a&b.png".to_string(),
            ],
            "an escaped key is unescaped, or asking for it back answers 404"
        );
        assert_eq!(
            tagged(body, "NextContinuationToken"),
            vec!["1ueGcxLPRx1Tr".to_string()]
        );
        assert!(tagged(body, "NoSuchTag").is_empty());
    }

    #[test]
    fn test_the_host_is_what_the_signature_names() {
        assert_eq!(
            host_of("https://s3.us-east-1.amazonaws.com"),
            "s3.us-east-1.amazonaws.com"
        );
        assert_eq!(host_of("http://localhost:9000"), "localhost:9000");
        assert_eq!(host_of("http://localhost:9000/ignored"), "localhost:9000");
    }

    #[test]
    fn test_a_prefix_is_given_the_slash_it_needs() {
        let store = |prefix: &str| {
            S3::new(
                "http://localhost:9000",
                "state",
                "us-east-1",
                "key",
                "secret",
            )
            .expect("a client")
            .under(prefix)
        };

        assert_eq!(store("fleet").at("boxes/x"), "fleet/boxes/x");
        assert_eq!(store("fleet/").at("boxes/x"), "fleet/boxes/x");
        assert_eq!(store("").at("boxes/x"), "boxes/x");
    }

    #[tokio::test]
    async fn test_a_bucket_behaves_like_a_blob_backend() {
        let Ok(store) = S3::from_env() else {
            return;
        };

        store.delete_prefix("boxes/").await.expect("a clean start");
        conformance::blobs(&store).await;
    }
}
