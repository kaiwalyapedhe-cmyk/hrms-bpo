//! Call-centre extensions — QA scorecards, aux/adherence state and CDR
//! ingestion. Every write that changes a KPI pushes a dashboard delta.

use axum::extract::{Multipart, Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::{AnyRole, AuthUser, QualityOffice};
use crate::error::{ApiError, ApiResult};
use crate::jobs::IngestSummary;
use crate::realtime::AgentStatus;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/quality/forms", get(forms_list).post(forms_create))
        .route("/quality/scores", get(scores_list).post(scores_create))
        .route("/aux/codes", get(aux_codes_list))
        .route("/aux/status", get(aux_status).post(aux_change))
        .route("/aux/history", get(aux_history))
        .route("/cdr/import", post(cdr_import))
        .route("/cdr", get(cdr_list))
}

// ---------------------------------------------------------------------------
// QA forms & scores
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct Criterion {
    pub id: Uuid,
    pub name: String,
    pub weight: Decimal,
    pub max_score: i32,
    pub is_csat: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct QaForm {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub active: bool,
    pub criteria: Vec<Criterion>,
}

#[utoipa::path(get, path = "/api/quality/forms", tag = "quality", responses((status = 200, body = Vec<QaForm>)))]
pub async fn forms_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<QaForm>>> {
    #[derive(FromRow)]
    struct FormRow {
        id: Uuid,
        name: String,
        description: Option<String>,
        active: bool,
    }
    let forms = sqlx::query_as::<_, FormRow>(
        "select id, name, description, active from qa_forms order by name",
    )
    .fetch_all(&state.db)
    .await?;

    let mut out = Vec::new();
    for form in forms {
        let criteria = sqlx::query_as::<_, Criterion>(
            "select id, name, weight, max_score, is_csat from qa_criteria where form_id = $1 order by name",
        )
        .bind(form.id)
        .fetch_all(&state.db)
        .await?;
        out.push(QaForm {
            id: form.id,
            name: form.name,
            description: form.description,
            active: form.active,
            criteria,
        });
    }
    Ok(Json(out))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CriterionInput {
    pub name: String,
    pub weight: Decimal,
    pub max_score: Option<i32>,
    pub is_csat: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct FormInput {
    pub name: String,
    pub description: Option<String>,
    pub criteria: Vec<CriterionInput>,
}

#[utoipa::path(post, path = "/api/quality/forms", tag = "quality", request_body = FormInput,
    responses((status = 200, body = QaForm)))]
pub async fn forms_create(
    _user: AuthUser<QualityOffice>,
    State(state): State<AppState>,
    Json(input): Json<FormInput>,
) -> ApiResult<Json<QaForm>> {
    if input.criteria.is_empty() {
        return Err(ApiError::bad("a QA form needs at least one criterion"));
    }
    let mut total_weight = Decimal::ZERO;
    for criterion in &input.criteria {
        if criterion.weight <= Decimal::ZERO {
            return Err(ApiError::bad("criterion weight must be positive"));
        }
        if criterion.max_score.unwrap_or(5) <= 0 {
            return Err(ApiError::bad("criterion max_score must be positive"));
        }
        total_weight += criterion.weight;
    }
    if total_weight != Decimal::from(100) {
        tracing::info!(%total_weight, "QA form weights do not total 100; scores are normalised by the sum of weights");
    }

    let form_id: Uuid = sqlx::query_scalar(
        "insert into qa_forms (name, description) values ($1,$2) returning id",
    )
    .bind(&input.name)
    .bind(&input.description)
    .fetch_one(&state.db)
    .await?;

    for criterion in &input.criteria {
        sqlx::query(
            "insert into qa_criteria (form_id, name, weight, max_score, is_csat) values ($1,$2,$3,$4,$5)",
        )
        .bind(form_id)
        .bind(&criterion.name)
        .bind(criterion.weight)
        .bind(criterion.max_score.unwrap_or(5))
        .bind(criterion.is_csat.unwrap_or(false))
        .execute(&state.db)
        .await?;
    }

    let criteria = sqlx::query_as::<_, Criterion>(
        "select id, name, weight, max_score, is_csat from qa_criteria where form_id = $1 order by name",
    )
    .bind(form_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(QaForm {
        id: form_id,
        name: input.name,
        description: input.description,
        active: true,
        criteria,
    }))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct QaScore {
    pub id: Uuid,
    pub employee_id: Uuid,
    pub employee_name: String,
    pub evaluator_name: Option<String>,
    pub form_name: Option<String>,
    pub total_score: Decimal,
    pub weighted_pct: Decimal,
    pub csat_score: Option<Decimal>,
    pub comments: Option<String>,
    pub evaluated_at: DateTime<Utc>,
}

const SCORE_SELECT: &str = r#"
select s.id, s.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
       (ev.first_name || ' ' || ev.last_name) as evaluator_name,
       f.name as form_name, s.total_score, s.weighted_pct, s.csat_score, s.comments, s.evaluated_at
from qa_scores s
join employees e on e.id = s.employee_id
left join employees ev on ev.id = s.evaluator_id
left join qa_forms f on f.id = s.form_id
"#;

#[derive(Debug, Deserialize, ToSchema)]
pub struct ScoreQuery {
    pub employee_id: Option<Uuid>,
    pub days: Option<i32>,
}

#[utoipa::path(get, path = "/api/quality/scores", tag = "quality",
    params(("employee_id" = Option<Uuid>, Query), ("days" = Option<i32>, Query)),
    responses((status = 200, body = Vec<QaScore>)))]
pub async fn scores_list(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Query(query): Query<ScoreQuery>,
) -> ApiResult<Json<Vec<QaScore>>> {
    let employee_id = match user.session.role {
        hrms_core::Role::Agent => user.session.employee_id,
        _ => query.employee_id,
    };
    let sql = format!(
        "{SCORE_SELECT} where ($1::uuid is null or s.employee_id = $1)
         and s.evaluated_at >= now() - ($2::int || ' days')::interval
         order by s.evaluated_at desc limit 300"
    );
    let rows = sqlx::query_as::<_, QaScore>(&sql)
        .bind(employee_id)
        .bind(query.days.unwrap_or(30).clamp(1, 365))
        .fetch_all(&state.db)
        .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ScoreInput {
    pub employee_id: Uuid,
    pub form_id: Uuid,
    pub cdr_id: Option<Uuid>,
    /// Criterion name → score awarded (raw, out of that criterion's max).
    pub scores: serde_json::Value,
    pub csat_score: Option<Decimal>,
    pub comments: Option<String>,
}

#[utoipa::path(post, path = "/api/quality/scores", tag = "quality", request_body = ScoreInput,
    responses((status = 200, body = QaScore), (status = 400, description = "invalid form or scores")))]
pub async fn scores_create(
    user: AuthUser<QualityOffice>,
    State(state): State<AppState>,
    Json(input): Json<ScoreInput>,
) -> ApiResult<Json<QaScore>> {
    if !input.scores.is_object() {
        return Err(ApiError::bad("scores must be an object of criterion → value"));
    }
    let criteria = sqlx::query_as::<_, Criterion>(
        "select id, name, weight, max_score, is_csat from qa_criteria where form_id = $1",
    )
    .bind(input.form_id)
    .fetch_all(&state.db)
    .await?;
    if criteria.is_empty() {
        return Err(ApiError::not_found("QA form criteria"));
    }

    let mut weighted_sum = Decimal::ZERO;
    let mut weight_total = Decimal::ZERO;
    let mut raw_total = Decimal::ZERO;
    for criterion in &criteria {
        let awarded = input
            .scores
            .get(&criterion.name)
            .and_then(|v| v.as_f64())
            .map(|v| Decimal::try_from(v).unwrap_or(Decimal::ZERO))
            .unwrap_or(Decimal::ZERO);
        if awarded < Decimal::ZERO || awarded > Decimal::from(criterion.max_score) {
            return Err(ApiError::bad(format!(
                "score for `{}` must be between 0 and {}",
                criterion.name, criterion.max_score
            )));
        }
        raw_total += awarded;
        weighted_sum += criterion.weight * awarded / Decimal::from(criterion.max_score);
        weight_total += criterion.weight;
    }
    let weighted_pct = if weight_total > Decimal::ZERO {
        (weighted_sum / weight_total * Decimal::from(100)).round_dp(2)
    } else {
        Decimal::ZERO
    };

    let id: Uuid = sqlx::query_scalar(
        r#"insert into qa_scores
           (employee_id, evaluator_id, cdr_id, form_id, scores, total_score, weighted_pct, csat_score, comments)
           values ($1,$2,$3,$4,$5,$6,$7,$8,$9) returning id"#,
    )
    .bind(input.employee_id)
    .bind(user.session.employee_id)
    .bind(input.cdr_id)
    .bind(input.form_id)
    .bind(&input.scores)
    .bind(raw_total)
    .bind(weighted_pct)
    .bind(input.csat_score)
    .bind(&input.comments)
    .fetch_one(&state.db)
    .await?;

    // CSAT + QA panels update on submission.
    let state_clone = state.clone();
    let employee_id = input.employee_id;
    tokio::spawn(async move {
        let _ = crate::metrics::refresh(&state_clone, false).await;
        if let Ok(personal) = crate::metrics::personal_metrics(&state_clone.db, employee_id).await {
            state_clone.hub.publish_personal(employee_id, 
                &personal
                    .iter()
                    .map(|(k, v)| (k.as_str(), *v))
                    .collect::<Vec<_>>(),
            );
        }
    });

    let sql = format!("{SCORE_SELECT} where s.id = $1");
    let row = sqlx::query_as::<_, QaScore>(&sql)
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    state
        .audit(Some(&user.session), "score", "qa_score", Some(id.to_string()), serde_json::json!({"weighted_pct": weighted_pct}))
        .await;
    Ok(Json(row))
}

// ---------------------------------------------------------------------------
// Aux codes / adherence
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct AuxCode {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub category: String,
    pub is_paid: bool,
    pub colour: String,
    pub active: bool,
}

#[utoipa::path(get, path = "/api/aux/codes", tag = "quality", responses((status = 200, body = Vec<AuxCode>)))]
pub async fn aux_codes_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<AuxCode>>> {
    let rows = sqlx::query_as::<_, AuxCode>(
        "select id, code, name, category, is_paid, colour, active from aux_codes where active order by code",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct AgentState {
    pub employee_id: Uuid,
    pub employee_name: String,
    pub aux_code: String,
    pub category: String,
    pub colour: String,
    pub since: DateTime<Utc>,
    pub minutes_in_state: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AuxStatusQuery {
    pub employee_id: Option<Uuid>,
}

/// Current aux state per agent (the dashboard status grid).
#[utoipa::path(get, path = "/api/aux/status", tag = "quality",
    params(("employee_id" = Option<Uuid>, Query)), responses((status = 200, body = Vec<AgentState>)))]
pub async fn aux_status(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Query(query): Query<AuxStatusQuery>,
) -> ApiResult<Json<Vec<AgentState>>> {
    let employee_id = match user.session.role {
        hrms_core::Role::Agent => user.session.employee_id,
        _ => query.employee_id,
    };
    let rows = sqlx::query_as::<_, AgentState>(
        r#"select e.id as employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  coalesce(ac.code,'OFFLINE') as aux_code,
                  coalesce(ac.category,'offline') as category,
                  coalesce(ac.colour,'gray') as colour,
                  coalesce(s.started_at, now()) as since,
                  (extract(epoch from (now() - coalesce(s.started_at, now()))) / 60)::bigint as minutes_in_state
           from employees e
           left join lateral (
               select a.id, a.aux_code_id, a.started_at from agent_aux_state a
               where a.employee_id = e.id and a.ended_at is null
               order by a.started_at desc limit 1
           ) s on true
           left join aux_codes ac on ac.id = s.aux_code_id
           where e.status in ('active','probation','on_leave','notice')
             and ($1::uuid is null or e.id = $1)
           order by employee_name"#,
    )
    .bind(employee_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AuxChangeInput {
    pub code: String,
    pub employee_id: Option<Uuid>,
}

/// Change an agent's aux state. Closes the open segment and publishes the new
/// colour to every connected dashboard immediately.
#[utoipa::path(post, path = "/api/aux/status", tag = "quality", request_body = AuxChangeInput,
    responses((status = 200, body = AgentState)))]
pub async fn aux_change(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Json(input): Json<AuxChangeInput>,
) -> ApiResult<Json<AgentState>> {
    let employee_id = match user.session.role {
        hrms_core::Role::Agent => user
            .session
            .employee_id
            .ok_or_else(|| ApiError::bad("agent account is not linked to an employee"))?,
        _ => input
            .employee_id
            .or(user.session.employee_id)
            .ok_or_else(|| ApiError::bad("employee_id is required"))?,
    };
    let aux: Option<(Uuid, String, String, String)> = sqlx::query_as(
        "select id, code, category, colour from aux_codes where code = $1 and active",
    )
    .bind(&input.code)
    .fetch_optional(&state.db)
    .await?;
    let (aux_id, code, category, colour) = aux.ok_or_else(|| ApiError::bad("unknown aux code"))?;

    let mut tx = state.db.begin().await?;
    sqlx::query("update agent_aux_state set ended_at = now() where employee_id = $1 and ended_at is null")
        .bind(employee_id)
        .execute(&mut *tx)
        .await?;
    let started_at: DateTime<Utc> = sqlx::query_scalar(
        "insert into agent_aux_state (employee_id, aux_code_id) values ($1,$2) returning started_at",
    )
    .bind(employee_id)
    .bind(aux_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    #[derive(FromRow)]
    struct NameRow {
        name: String,
    }
    let name: NameRow = sqlx::query_as(
        "select (first_name || ' ' || last_name) as name from employees where id = $1",
    )
    .bind(employee_id)
    .fetch_one(&state.db)
    .await?;

    state.hub.publish_agent(AgentStatus {
        employee_id,
        employee_name: name.name.clone(),
        aux_code: code.clone(),
        category: category.clone(),
        colour: colour.clone(),
        on_call: category == "on_call",
        since: started_at,
    });
    let state_clone = state.clone();
    tokio::spawn(async move {
        let _ = crate::metrics::refresh(&state_clone, false).await;
    });

    Ok(Json(AgentState {
        employee_id,
        employee_name: name.name,
        aux_code: code,
        category,
        colour,
        since: started_at,
        minutes_in_state: 0,
    }))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct AuxSegment {
    pub aux_code: String,
    pub category: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub minutes: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AuxHistoryQuery {
    pub employee_id: Option<Uuid>,
    pub hours: Option<i32>,
}

#[utoipa::path(get, path = "/api/aux/history", tag = "quality",
    params(("employee_id" = Option<Uuid>, Query), ("hours" = Option<i32>, Query)),
    responses((status = 200, body = Vec<AuxSegment>)))]
pub async fn aux_history(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Query(query): Query<AuxHistoryQuery>,
) -> ApiResult<Json<Vec<AuxSegment>>> {
    let employee_id = match user.session.role {
        hrms_core::Role::Agent => user.session.employee_id,
        _ => query.employee_id,
    };
    let rows = sqlx::query_as::<_, AuxSegment>(
        r#"select ac.code as aux_code, ac.category, s.started_at, s.ended_at,
                  (extract(epoch from (coalesce(s.ended_at, now()) - s.started_at)) / 60)::bigint as minutes
           from agent_aux_state s join aux_codes ac on ac.id = s.aux_code_id
           where ($1::uuid is null or s.employee_id = $1)
             and s.started_at >= now() - ($2::int || ' hours')::interval
           order by s.started_at desc limit 500"#,
    )
    .bind(employee_id)
    .bind(query.hours.unwrap_or(8).clamp(1, 72))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

// ---------------------------------------------------------------------------
// CDR
// ---------------------------------------------------------------------------

/// Upload a FreeSWITCH/Asterisk CDR CSV (multipart field `file`). Rows are
/// de-duplicated on `uniqueid`; `agent_ext` maps to `employees.extension`.
#[utoipa::path(post, path = "/api/cdr/import", tag = "quality",
    request_body(content = String, content_type = "multipart/form-data", description = "CSV file field `file`"),
    responses((status = 200, body = IngestSummary)))]
pub async fn cdr_import(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> ApiResult<Json<IngestSummary>> {
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad(format!("multipart: {e}")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field.file_name().unwrap_or("cdr.csv").to_string();
        let bytes = field
            .bytes()
            .await
            .map_err(|e| ApiError::bad(format!("file read: {e}")))?;
        let summary = crate::jobs::ingest_cdr_csv(&state, &bytes, &filename)
            .await
            .map_err(ApiError::bad)?;
        state
            .audit(
                Some(&user.session),
                "cdr_upload",
                "cdr",
                None,
                serde_json::json!({"file": filename, "inserted": summary.inserted}),
            )
            .await;
        return Ok(Json(summary));
    }
    Err(ApiError::bad("missing multipart field `file`"))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct CdrRow {
    pub id: Uuid,
    pub uniqueid: String,
    pub call_start: DateTime<Utc>,
    pub direction: String,
    pub queue: Option<String>,
    pub agent_ext: Option<String>,
    pub employee_name: Option<String>,
    pub disposition: Option<String>,
    pub wait_seconds: i32,
    pub talk_seconds: i32,
    pub hold_seconds: i32,
    pub wrap_seconds: i32,
    pub abandoned: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CdrQuery {
    pub date: Option<chrono::NaiveDate>,
    pub employee_id: Option<Uuid>,
    pub limit: Option<i64>,
}

#[utoipa::path(get, path = "/api/cdr", tag = "quality",
    params(("date" = Option<chrono::NaiveDate>, Query), ("employee_id" = Option<Uuid>, Query),
           ("limit" = Option<i64>, Query)),
    responses((status = 200, body = Vec<CdrRow>)))]
pub async fn cdr_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Query(query): Query<CdrQuery>,
) -> ApiResult<Json<Vec<CdrRow>>> {
    let rows = sqlx::query_as::<_, CdrRow>(
        r#"select c.id, c.uniqueid, c.call_start, c.direction, c.queue, c.agent_ext,
                  (e.first_name || ' ' || e.last_name) as employee_name, c.disposition,
                  c.wait_seconds, c.talk_seconds, c.hold_seconds, c.wrap_seconds, c.abandoned
           from cdr_records c left join employees e on e.id = c.employee_id
           where ($1::date is null or c.call_start::date = $1)
             and ($2::uuid is null or c.employee_id = $2)
           order by c.call_start desc limit $3"#,
    )
    .bind(query.date)
    .bind(query.employee_id)
    .bind(query.limit.unwrap_or(100).clamp(1, 1000))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}
