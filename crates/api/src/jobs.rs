//! Background job worker (Redis + fred) and FreeSWITCH/Asterisk CDR ingestion.
//!
//! CDR ingestion is available synchronously over HTTP as well; when the file is
//! large the API stores it in MinIO and enqueues `cdr_ingest` instead.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::queue::Job;
use crate::state::AppState;

#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct IngestSummary {
    pub rows_read: usize,
    pub inserted: usize,
    pub duplicates: usize,
    pub unmapped_agents: usize,
    pub errors: Vec<String>,
}

fn parse_ts(raw: &str) -> Option<DateTime<Utc>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&Utc));
    }
    for format in [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%d/%m/%Y %H:%M:%S",
        "%d/%m/%Y %H:%M",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(raw, format) {
            return Some(Utc.from_utc_datetime(&naive));
        }
    }
    NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|naive| Utc.from_utc_datetime(&naive))
}

fn int_field(raw: Option<&String>) -> i32 {
    raw.and_then(|v| v.trim().parse::<i32>().ok())
        .unwrap_or(0)
        .max(0)
}

fn bool_field(raw: Option<&String>) -> bool {
    matches!(
        raw.map(|v| v.trim().to_lowercase()).as_deref(),
        Some("true") | Some("yes") | Some("1") | Some("y")
    )
}

/// Parse and persist a CDR CSV. Rows are de-duplicated on `uniqueid`, and
/// `agent_ext` is resolved to an employee through `employees.extension`.
pub async fn ingest_cdr_csv(
    state: &AppState,
    bytes: &[u8],
    source_file: &str,
) -> Result<IngestSummary, String> {
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .flexible(true)
        .from_reader(bytes);
    let headers = reader.headers().cloned().unwrap_or_default();

    let index = |name: &str| -> Option<usize> {
        headers.iter().position(|h| h.eq_ignore_ascii_case(name))
    };
    let col = |record: &csv::StringRecord, name: &str| -> Option<String> {
        index(name)
            .and_then(|i| record.get(i))
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };

    if index("uniqueid").is_none() || index("call_start").is_none() {
        return Err("CDR file must contain `uniqueid` and `call_start` columns".into());
    }

    let mut summary = IngestSummary::default();
    let mut agent_cache: HashMap<String, Option<Uuid>> = HashMap::new();

    for (offset, record) in reader.records().enumerate() {
        let line = offset + 2;
        let record = match record {
            Ok(r) => r,
            Err(err) => {
                summary.errors.push(format!("line {line}: {err}"));
                continue;
            }
        };
        summary.rows_read += 1;

        let Some(uniqueid) = col(&record, "uniqueid") else {
            summary.errors.push(format!("line {line}: missing uniqueid"));
            continue;
        };
        let Some(call_start) = col(&record, "call_start").and_then(|v| parse_ts(&v)) else {
            summary
                .errors
                .push(format!("line {line}: missing or unparseable call_start"));
            continue;
        };

        let agent_ext = col(&record, "agent_ext");
        let employee_id = match &agent_ext {
            Some(ext) => {
                if let Some(cached) = agent_cache.get(ext) {
                    *cached
                } else {
                    let found: Option<Uuid> = sqlx::query_scalar(
                        "select id from employees where extension = $1",
                    )
                    .bind(ext)
                    .fetch_optional(&state.db)
                    .await
                    .map_err(|e| e.to_string())?;
                    agent_cache.insert(ext.clone(), found);
                    found
                }
            }
            None => None,
        };
        if agent_ext.is_some() && employee_id.is_none() {
            summary.unmapped_agents += 1;
        }

        let direction = col(&record, "direction")
            .map(|d| d.to_lowercase())
            .filter(|d| ["inbound", "outbound", "internal"].contains(&d.as_str()))
            .unwrap_or_else(|| "inbound".into());

        let result = sqlx::query(
            r#"insert into cdr_records
               (uniqueid, call_start, call_answer, call_end, direction, queue, agent_ext, employee_id,
                disposition, wait_seconds, talk_seconds, hold_seconds, wrap_seconds, abandoned,
                customer_number, source_file)
               values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)
               on conflict (uniqueid) do nothing"#,
        )
        .bind(&uniqueid)
        .bind(call_start)
        .bind(col(&record, "call_answer").and_then(|v| parse_ts(&v)))
        .bind(col(&record, "call_end").and_then(|v| parse_ts(&v)))
        .bind(&direction)
        .bind(col(&record, "queue"))
        .bind(&agent_ext)
        .bind(employee_id)
        .bind(col(&record, "disposition"))
        .bind(int_field(col(&record, "wait_seconds").as_ref()))
        .bind(int_field(col(&record, "talk_seconds").as_ref()))
        .bind(int_field(col(&record, "hold_seconds").as_ref()))
        .bind(int_field(col(&record, "wrap_seconds").as_ref()))
        .bind(bool_field(col(&record, "abandoned").as_ref()))
        .bind(col(&record, "customer_number"))
        .bind(source_file)
        .execute(&state.db)
        .await;

        match result {
            Ok(done) if done.rows_affected() == 1 => summary.inserted += 1,
            Ok(_) => summary.duplicates += 1,
            Err(err) => summary.errors.push(format!("line {line}: {err}")),
        }
    }

    // A new CDR changes AHT, queue wait and occupancy: push fresh values now.
    if let Err(err) = crate::metrics::refresh(state, false).await {
        tracing::warn!("metrics refresh after CDR ingest failed: {err}");
    }
    state
        .audit(
            None,
            "cdr_ingest",
            "cdr",
            None,
            serde_json::json!({
                "source_file": source_file,
                "rows": summary.rows_read,
                "inserted": summary.inserted,
            }),
        )
        .await;

    Ok(summary)
}

/// Drain the Redis queue forever. Falls back to polling when empty.
pub fn spawn_worker(state: AppState) {
    if !state.queue.enabled() {
        tracing::info!("background worker not started (no Redis)");
        return;
    }
    tokio::spawn(async move {
        tracing::info!("background job worker started");
        loop {
            match state.queue.pop().await {
                Some(job) => {
                    tracing::info!(kind = job.label(), "processing job");
                    handle_job(&state, job).await;
                }
                None => tokio::time::sleep(Duration::from_millis(750)).await,
            }
        }
    });
}

async fn handle_job(state: &AppState, job: Job) {
    match job {
        Job::CdrIngest {
            object_key,
            source_file,
        } => {
            if !state.storage.enabled() {
                tracing::warn!("cdr_ingest job skipped: object storage disabled");
                return;
            }
            match state.storage.get(&object_key).await {
                Ok(bytes) => match ingest_cdr_csv(state, &bytes, &source_file).await {
                    Ok(summary) => tracing::info!(
                        inserted = summary.inserted,
                        duplicates = summary.duplicates,
                        "cdr ingest finished"
                    ),
                    Err(err) => tracing::warn!("cdr ingest failed: {err}"),
                },
                Err(err) => tracing::warn!("could not fetch {object_key}: {err}"),
            }
        }
        Job::MetricsRefresh => crate::metrics::tick(state).await,
    }
}
