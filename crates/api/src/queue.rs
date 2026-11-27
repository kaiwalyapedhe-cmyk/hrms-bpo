//! Redis-backed job queue (fred). Payroll runs are synchronous and
//! transactional; heavy or retryable work — CDR file ingestion, compliance
//! exports, KPI recomputation — is pushed here and drained by a worker task.

use std::sync::Arc;

use fred::prelude::*;
use serde::{Deserialize, Serialize};

pub const QUEUE_KEY: &str = "hrms:jobs";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Job {
    CdrIngest {
        object_key: String,
        source_file: String,
    },
    MetricsRefresh,
}

impl Job {
    pub fn label(&self) -> &'static str {
        match self {
            Job::CdrIngest { .. } => "cdr_ingest",
            Job::MetricsRefresh => "metrics_refresh",
        }
    }
}

pub struct JobQueue {
    client: Option<Arc<Client>>,
    url: String,
}

impl JobQueue {
    pub async fn connect(url: &str) -> Self {
        if url.trim().is_empty() {
            tracing::warn!("REDIS_URL not set — background queue disabled");
            return JobQueue {
                client: None,
                url: String::new(),
            };
        }
        let config = match Config::from_url(url) {
            Ok(c) => c,
            Err(err) => {
                tracing::warn!("invalid REDIS_URL ({err}); background queue disabled");
                return JobQueue {
                    client: None,
                    url: url.to_string(),
                };
            }
        };
        let client = match Builder::from_config(config).build() {
            Ok(c) => c,
            Err(err) => {
                tracing::warn!("could not build redis client ({err}); queue disabled");
                return JobQueue {
                    client: None,
                    url: url.to_string(),
                };
            }
        };
        if let Err(err) = client.init().await {
            tracing::warn!("redis unavailable at {url} ({err}); queue disabled");
            return JobQueue {
                client: None,
                url: url.to_string(),
            };
        }
        tracing::info!("redis job queue connected at {url}");
        JobQueue {
            client: Some(Arc::new(client)),
            url: url.to_string(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.client.is_some()
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub async fn enqueue(&self, job: &Job) -> Result<(), String> {
        let client = self.client.as_ref().ok_or("queue disabled")?;
        let payload = serde_json::to_string(job).map_err(|e| e.to_string())?;
        let _: i64 = client
            .lpush(QUEUE_KEY, payload)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn pop(&self) -> Option<Job> {
        let client = self.client.as_ref()?;
        let raw: Option<String> = client.rpop(QUEUE_KEY, None).await.ok()?;
        raw.and_then(|value| match serde_json::from_str(&value) {
            Ok(job) => Some(job),
            Err(err) => {
                tracing::warn!("discarding malformed job payload: {err}");
                None
            }
        })
    }

    pub async fn depth(&self) -> i64 {
        let Some(client) = self.client.as_ref() else {
            return 0;
        };
        client.llen(QUEUE_KEY).await.unwrap_or(0)
    }
}
