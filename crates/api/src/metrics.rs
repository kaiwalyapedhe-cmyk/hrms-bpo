//! KPI computation. Every number published on the wallboard is derived here
//! from the operational tables (punches, aux state, CDRs, QA scores), so the
//! live dashboard and the 60-second history snapshots can never disagree.

use sqlx::PgPool;
use uuid::Uuid;

use crate::realtime::{AgentStatus, Audience};
use crate::state::AppState;

async fn scalar(db: &PgPool, sql: &str) -> Result<f64, sqlx::Error> {
    sqlx::query_scalar::<_, f64>(sql).fetch_one(db).await
}

const CLOCKED_IN_SQL: &str = r#"
select count(*)::float8 from (
    select distinct on (employee_id) employee_id, punch_type
    from punches
    where punch_time::date = current_date
    order by employee_id, punch_time desc
) latest
where punch_type = 'in'
"#;

const SCHEDULED_SQL: &str = r#"
select count(*)::float8 from shift_assignments
where work_date = current_date
  and exists (select 1 from employees e where e.id = shift_assignments.employee_id and e.status in ('active','probation','notice'))
"#;

const ADHERENCE_SQL: &str = r#"
select coalesce(100.0 * count(*) filter (where ac.category in ('available','on_call'))
       / nullif(count(*), 0), 0)::float8
from agent_aux_state s
join aux_codes ac on ac.id = s.aux_code_id
where s.ended_at is null
"#;

const AHT_15_SQL: &str = r#"
select coalesce(avg(talk_seconds + hold_seconds + wrap_seconds), 0)::float8
from cdr_records
where call_start >= now() - interval '15 minutes' and not abandoned
"#;

const AHT_30_SQL: &str = r#"
select coalesce(avg(talk_seconds + hold_seconds + wrap_seconds), 0)::float8
from cdr_records
where call_start >= now() - interval '30 minutes' and not abandoned
"#;

const QUEUE_WAIT_SQL: &str = r#"
select coalesce(round(avg(wait_seconds)), 0)::float8
from cdr_records
where call_start >= now() - interval '15 minutes'
"#;

const CSAT_SQL: &str = r#"
select coalesce(avg(csat_score), 0)::float8
from qa_scores
where evaluated_at >= now() - interval '24 hours' and csat_score is not null
"#;

const QA_SCORE_SQL: &str = r#"
select coalesce(avg(weighted_pct), 0)::float8
from qa_scores
where evaluated_at >= now() - interval '7 days'
"#;

const OCCUPANCY_SQL: &str = r#"
select coalesce(least(100.0, 100.0 * coalesce(sum(talk_seconds + hold_seconds + wrap_seconds), 0)
       / nullif((select count(*) from (
             select distinct on (employee_id) employee_id, punch_type
             from punches where punch_time::date = current_date
             order by employee_id, punch_time desc) latest
          where punch_type = 'in') * 1800.0, 0)), 0)::float8
from cdr_records
where call_start >= now() - interval '30 minutes' and not abandoned
"#;

const CALL_STATS_SQL: &str = r#"
select count(*)::float8,
       count(*) filter (where not abandoned)::float8,
       count(*) filter (where abandoned)::float8
from cdr_records
where call_start::date = current_date
"#;

/// All live metrics. Keys are the contract with the dashboard frontend.
pub async fn compute_metrics(db: &PgPool) -> Result<Vec<(&'static str, f64)>, sqlx::Error> {
    let clocked_in = scalar(db, CLOCKED_IN_SQL).await?;
    let scheduled = scalar(db, SCHEDULED_SQL).await?;
    let adherence = scalar(db, ADHERENCE_SQL).await?;
    let aht15 = scalar(db, AHT_15_SQL).await?;
    let aht30 = scalar(db, AHT_30_SQL).await?;
    let queue_wait = scalar(db, QUEUE_WAIT_SQL).await?;
    let csat = scalar(db, CSAT_SQL).await?;
    let qa = scalar(db, QA_SCORE_SQL).await?;
    let occupancy = scalar(db, OCCUPANCY_SQL).await?;
    let (offered, answered, abandoned): (f64, f64, f64) = sqlx::query_as(CALL_STATS_SQL)
        .fetch_one(db)
        .await?;

    let shrinkage = if scheduled > 0.0 {
        ((scheduled - clocked_in) / scheduled * 100.0).max(0.0)
    } else {
        0.0
    };

    Ok(vec![
        ("clocked_in", clocked_in),
        ("scheduled", scheduled),
        ("schedule_adherence", adherence),
        ("aht_15m", aht15),
        ("aht_30m", aht30),
        ("queue_wait", queue_wait),
        ("csat_avg", csat),
        ("qa_score_avg", qa),
        ("occupancy_pct", occupancy),
        ("shrinkage_pct", shrinkage),
        ("calls_offered", offered),
        ("calls_answered", answered),
        ("abandoned_calls", abandoned),
    ])
}

/// Current aux state per agent (the grid at the bottom of the wallboard).
pub async fn agent_statuses(db: &PgPool) -> Result<Vec<AgentStatus>, sqlx::Error> {
    sqlx::query_as::<_, AgentStatus>(
        r#"
        select e.id as employee_id,
               (e.first_name || ' ' || e.last_name) as employee_name,
               coalesce(ac.code, 'OFFLINE') as aux_code,
               coalesce(ac.category, 'offline') as category,
               coalesce(ac.colour, 'gray') as colour,
               coalesce(ac.category = 'on_call', false) as on_call,
               coalesce(s.started_at, now()) as since
        from employees e
        left join lateral (
            select a.id, a.aux_code_id, a.started_at
            from agent_aux_state a
            where a.employee_id = e.id and a.ended_at is null
            order by a.started_at desc limit 1
        ) s on true
        left join aux_codes ac on ac.id = s.aux_code_id
        where e.status in ('active','probation','on_leave','notice')
        order by e.first_name, e.last_name
        "#,
    )
    .fetch_all(db)
    .await
}

/// Personal metrics for the Agent self-service view.
pub async fn personal_metrics(
    db: &PgPool,
    employee_id: Uuid,
) -> Result<Vec<(String, f64)>, sqlx::Error> {
    let my_aht: f64 = sqlx::query_scalar(
        r#"select coalesce(avg(talk_seconds + hold_seconds + wrap_seconds),0)::float8
           from cdr_records where employee_id = $1 and call_start >= now() - interval '24 hours'"#,
    )
    .bind(employee_id)
    .fetch_one(db)
    .await?;
    let my_csat: f64 = sqlx::query_scalar(
        r#"select coalesce(avg(csat_score),0)::float8
           from qa_scores where employee_id = $1 and evaluated_at >= now() - interval '7 days'"#,
    )
    .bind(employee_id)
    .fetch_one(db)
    .await?;
    let my_calls: f64 = sqlx::query_scalar(
        r#"select count(*)::float8 from cdr_records
           where employee_id = $1 and call_start::date = current_date"#,
    )
    .bind(employee_id)
    .fetch_one(db)
    .await?;
    let my_qa: f64 = sqlx::query_scalar(
        r#"select coalesce(avg(weighted_pct),0)::float8
           from qa_scores where employee_id = $1 and evaluated_at >= now() - interval '7 days'"#,
    )
    .bind(employee_id)
    .fetch_one(db)
    .await?;

    Ok(vec![
        ("my_aht".to_string(), my_aht),
        ("my_csat".to_string(), my_csat),
        ("my_calls_today".to_string(), my_calls),
        ("my_qa_score".to_string(), my_qa),
    ])
}

/// Recompute, persist a snapshot row and push changed metrics to clients.
pub async fn tick(state: &AppState) {
    match refresh(state, true).await {
        Ok(()) => {}
        Err(err) => tracing::warn!("metrics tick failed: {err}"),
    }
}

pub async fn refresh(state: &AppState, persist: bool) -> Result<(), sqlx::Error> {
    let metrics = compute_metrics(&state.db).await?;

    let quality: Vec<(&str, f64)> = metrics
        .iter()
        .filter(|(k, _)| k.starts_with("csat") || k.starts_with("qa_") || k.starts_with("aht"))
        .copied()
        .collect();
    let rest: Vec<(&str, f64)> = metrics
        .iter()
        .filter(|(k, _)| !k.starts_with("csat") && !k.starts_with("qa_") && !k.starts_with("aht"))
        .copied()
        .collect();

    state.hub.publish_metrics(&rest, Audience::Full);
    state.hub.publish_metrics(&rest, Audience::Workforce);
    state.hub.publish_metrics(&quality, Audience::QualityOnly);
    state.hub.publish_metrics(&quality, Audience::Full);

    let agents = agent_statuses(&state.db).await?;
    state.hub.seed_agents(agents);

    if persist {
        let value = |key: &str| {
            metrics
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| *v)
                .unwrap_or(0.0)
        };
        sqlx::query(
            r#"insert into kpi_snapshots
               (clocked_in, scheduled, schedule_adherence, aht_15m, aht_30m, queue_wait,
                csat_avg, occupancy_pct, shrinkage_pct, calls_offered, calls_answered, abandoned_calls)
               values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)"#,
        )
        .bind(value("clocked_in") as i32)
        .bind(value("scheduled") as i32)
        .bind(value("schedule_adherence"))
        .bind(value("aht_15m"))
        .bind(value("aht_30m"))
        .bind(value("queue_wait") as i32)
        .bind(value("csat_avg"))
        .bind(value("occupancy_pct"))
        .bind(value("shrinkage_pct"))
        .bind(value("calls_offered") as i32)
        .bind(value("calls_answered") as i32)
        .bind(value("abandoned_calls") as i32)
        .execute(&state.db)
        .await?;
    }

    Ok(())
}
