//! MinIO / S3 object storage for payslip PDFs, statutory files and CDR uploads.
//! The service boots without object storage (S3_ENDPOINT unset) and falls back
//! to streaming generated files straight to the caller.

use aws_config::BehaviorVersion;
use aws_sdk_s3::config::{Credentials, Region};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;

pub struct ObjectStorage {
    client: Option<Client>,
    bucket: String,
}

impl ObjectStorage {
    pub async fn connect() -> Self {
        let endpoint = std::env::var("S3_ENDPOINT").unwrap_or_default();
        if endpoint.trim().is_empty() {
            tracing::warn!("S3_ENDPOINT not set — object storage disabled, files will stream inline");
            return ObjectStorage {
                client: None,
                bucket: String::new(),
            };
        }
        let bucket = std::env::var("S3_BUCKET").unwrap_or_else(|_| "hrms".to_string());
        let access = std::env::var("S3_ACCESS_KEY").unwrap_or_else(|_| "minioadmin".to_string());
        let secret = std::env::var("S3_SECRET_KEY").unwrap_or_else(|_| "minioadmin".to_string());
        let region = std::env::var("S3_REGION").unwrap_or_else(|_| "us-east-1".to_string());

        let shared = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(region))
            .endpoint_url(endpoint.clone())
            .credentials_provider(Credentials::new(
                access, secret, None, None, "hrms-static",
            ))
            .load()
            .await;
        let conf = aws_sdk_s3::config::Builder::from(&shared)
            .force_path_style(true)
            .build();
        let client = Client::from_conf(conf);

        match client.create_bucket().bucket(&bucket).send().await {
            Ok(_) => tracing::info!("object storage ready at {endpoint}/{bucket}"),
            Err(err) => tracing::warn!(
                "could not ensure bucket {bucket} at {endpoint} ({err}); uploads will be retried per request"
            ),
        }

        ObjectStorage {
            client: Some(client),
            bucket,
        }
    }

    pub fn enabled(&self) -> bool {
        self.client.is_some()
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// Upload and return the object key. Errors are stringly because callers
    /// either degrade gracefully or stream the bytes inline.
    pub async fn put(
        &self,
        key: &str,
        bytes: Vec<u8>,
        content_type: &str,
    ) -> Result<String, String> {
        let client = self.client.as_ref().ok_or("object storage disabled")?;
        client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .body(ByteStream::from(bytes))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Ok(key.to_string())
    }

    pub async fn get(&self, key: &str) -> Result<Vec<u8>, String> {
        let client = self.client.as_ref().ok_or("object storage disabled")?;
        let object = client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let data = object
            .body
            .collect()
            .await
            .map_err(|e| e.to_string())?
            .into_bytes()
            .to_vec();
        Ok(data)
    }
}
