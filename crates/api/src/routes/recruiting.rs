//! Recruiting Solutions — Talent Acquisition Manager (requisitions, pipeline,
//! interviews, offers) and the Candidate Gateway (public careers endpoints).

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::{AnyRole, AuthUser, RecruitingOffice};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/recruiting/requisitions", get(requisitions_list).post(requisitions_create))
        .route("/recruiting/requisitions/{id}/status", post(requisition_status))
        .route("/recruiting/candidates", get(candidates_list).post(candidates_create))
        .route("/recruiting/candidates/{id}/stage", post(candidate_stage))
        .route("/recruiting/interviews", get(interviews_list).post(interviews_create))
        .route("/recruiting/interviews/{id}/feedback", post(interview_feedback))
        .route("/recruiting/offers", get(offers_list).post(offers_create))
        .route("/recruiting/offers/{id}/respond", post(offer_respond))
        .route("/careers/requisitions", get(careers_requisitions))
        .route("/careers/apply", post(careers_apply))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct Requisition {
    pub id: Uuid,
    pub req_no: String,
    pub title: String,
    pub department_name: Option<String>,
    pub location_name: Option<String>,
    pub openings: i32,
    pub status: String,
    pub priority: String,
    pub opened_on: NaiveDate,
    pub target_date: Option<NaiveDate>,
    pub budget_annual: Decimal,
    pub candidate_count: i64,
}

const REQUISITION_SELECT: &str = r#"
select r.id, r.req_no, r.title, d.name as department_name, l.name as location_name,
       r.openings, r.status, r.priority, r.opened_on, r.target_date, r.budget_annual,
       (select count(*) from candidates c where c.requisition_id = r.id) as candidate_count
from requisitions r
left join departments d on d.id = r.department_id
left join locations l on l.id = r.location_id
"#;

#[derive(Debug, Deserialize, ToSchema)]
pub struct RequisitionQuery {
    pub status: Option<String>,
}

#[utoipa::path(get, path = "/api/recruiting/requisitions", tag = "recruiting",
    params(("status" = Option<String>, Query)), responses((status = 200, body = Vec<Requisition>)))]
pub async fn requisitions_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Query(query): Query<RequisitionQuery>,
) -> ApiResult<Json<Vec<Requisition>>> {
    let sql = format!("{REQUISITION_SELECT} where ($1::text is null or r.status = $1) order by r.opened_on desc");
    let rows = sqlx::query_as::<_, Requisition>(&sql)
        .bind(query.status)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct RequisitionInput {
    pub title: String,
    pub department_id: Option<Uuid>,
    pub location_id: Option<Uuid>,
    pub openings: Option<i32>,
    pub priority: Option<String>,
    pub target_date: Option<NaiveDate>,
    pub budget_annual: Option<Decimal>,
    pub job_description: Option<String>,
}

#[utoipa::path(post, path = "/api/recruiting/requisitions", tag = "recruiting",
    request_body = RequisitionInput, responses((status = 200, body = Requisition)))]
pub async fn requisitions_create(
    user: AuthUser<RecruitingOffice>,
    State(state): State<AppState>,
    Json(input): Json<RequisitionInput>,
) -> ApiResult<Json<Requisition>> {
    let req_no: String = sqlx::query_scalar("select 'REQ-' || to_char(now(),'YYYYMMDD') || '-' || lpad((count(*)+1)::text, 3, '0') from requisitions")
        .fetch_one(&state.db)
        .await?;
    let id: Uuid = sqlx::query_scalar(
        r#"insert into requisitions
           (req_no, title, department_id, location_id, openings, priority, target_date, budget_annual, job_description, recruiter_id)
           values ($1,$2,$3,$4,coalesce($5,1),coalesce($6,'medium'),$7,coalesce($8,0),$9,$10) returning id"#,
    )
    .bind(&req_no)
    .bind(&input.title)
    .bind(input.department_id)
    .bind(input.location_id)
    .bind(input.openings)
    .bind(&input.priority)
    .bind(input.target_date)
    .bind(input.budget_annual)
    .bind(&input.job_description)
    .bind(user.session.employee_id)
    .fetch_one(&state.db)
    .await?;
    let sql = format!("{REQUISITION_SELECT} where r.id = $1");
    let row = sqlx::query_as::<_, Requisition>(&sql)
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(row))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct StatusInput {
    pub status: String,
}

#[utoipa::path(post, path = "/api/recruiting/requisitions/{id}/status", tag = "recruiting",
    params(("id" = Uuid, Path)), request_body = StatusInput,
    responses((status = 200, body = Requisition)))]
pub async fn requisition_status(
    _user: AuthUser<RecruitingOffice>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<StatusInput>,
) -> ApiResult<Json<Requisition>> {
    if !["draft", "open", "on_hold", "filled", "cancelled"].contains(&input.status.as_str()) {
        return Err(ApiError::bad("invalid requisition status"));
    }
    sqlx::query("update requisitions set status = $2 where id = $1")
        .bind(id)
        .bind(&input.status)
        .execute(&state.db)
        .await?;
    let sql = format!("{REQUISITION_SELECT} where r.id = $1");
    let row = sqlx::query_as::<_, Requisition>(&sql)
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(row))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct Candidate {
    pub id: Uuid,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub phone: Option<String>,
    pub source: String,
    pub requisition_id: Option<Uuid>,
    pub req_no: Option<String>,
    pub stage: String,
    pub rating: Option<Decimal>,
    pub created_at: DateTime<Utc>,
}

const CANDIDATE_SELECT: &str = r#"
select c.id, c.first_name, c.last_name, c.email, c.phone, c.source, c.requisition_id,
       r.req_no, c.stage, c.rating, c.created_at
from candidates c left join requisitions r on r.id = c.requisition_id
"#;

#[derive(Debug, Deserialize, ToSchema)]
pub struct CandidateQuery {
    pub requisition_id: Option<Uuid>,
    pub stage: Option<String>,
}

#[utoipa::path(get, path = "/api/recruiting/candidates", tag = "recruiting",
    params(("requisition_id" = Option<Uuid>, Query), ("stage" = Option<String>, Query)),
    responses((status = 200, body = Vec<Candidate>)))]
pub async fn candidates_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Query(query): Query<CandidateQuery>,
) -> ApiResult<Json<Vec<Candidate>>> {
    let sql = format!(
        "{CANDIDATE_SELECT} where ($1::uuid is null or c.requisition_id = $1)
         and ($2::text is null or c.stage = $2) order by c.created_at desc limit 300"
    );
    let rows = sqlx::query_as::<_, Candidate>(&sql)
        .bind(query.requisition_id)
        .bind(query.stage)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CandidateInput {
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub phone: Option<String>,
    pub requisition_id: Option<Uuid>,
    pub source: Option<String>,
    pub resume_key: Option<String>,
}

#[utoipa::path(post, path = "/api/recruiting/candidates", tag = "recruiting",
    request_body = CandidateInput, responses((status = 200, body = Candidate)))]
pub async fn candidates_create(
    _user: AuthUser<RecruitingOffice>,
    State(state): State<AppState>,
    Json(input): Json<CandidateInput>,
) -> ApiResult<Json<Candidate>> {
    let id: Uuid = sqlx::query_scalar(
        r#"insert into candidates (first_name, last_name, email, phone, requisition_id, source, resume_key)
           values ($1,$2,$3,$4,$5,coalesce($6,'portal'),$7) returning id"#,
    )
    .bind(&input.first_name)
    .bind(&input.last_name)
    .bind(&input.email)
    .bind(&input.phone)
    .bind(input.requisition_id)
    .bind(&input.source)
    .bind(&input.resume_key)
    .fetch_one(&state.db)
    .await?;
    let sql = format!("{CANDIDATE_SELECT} where c.id = $1");
    let row = sqlx::query_as::<_, Candidate>(&sql)
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(row))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct StageInput {
    pub stage: String,
    pub rating: Option<Decimal>,
}

#[utoipa::path(post, path = "/api/recruiting/candidates/{id}/stage", tag = "recruiting",
    params(("id" = Uuid, Path)), request_body = StageInput,
    responses((status = 200, body = Candidate)))]
pub async fn candidate_stage(
    user: AuthUser<RecruitingOffice>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<StageInput>,
) -> ApiResult<Json<Candidate>> {
    if !["applied", "screening", "interview", "offer", "hired", "rejected", "withdrawn"]
        .contains(&input.stage.as_str())
    {
        return Err(ApiError::bad("invalid pipeline stage"));
    }
    sqlx::query("update candidates set stage = $2, rating = coalesce($3, rating) where id = $1")
        .bind(id)
        .bind(&input.stage)
        .bind(input.rating)
        .execute(&state.db)
        .await?;
    let sql = format!("{CANDIDATE_SELECT} where c.id = $1");
    let row = sqlx::query_as::<_, Candidate>(&sql)
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    state
        .audit(Some(&user.session), "stage_change", "candidate", Some(id.to_string()), serde_json::json!({"stage": input.stage}))
        .await;
    Ok(Json(row))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct Interview {
    pub id: Uuid,
    pub candidate_id: Uuid,
    pub candidate_name: String,
    pub requisition_id: Option<Uuid>,
    pub round: i32,
    pub scheduled_at: DateTime<Utc>,
    pub mode: String,
    pub interviewer_name: Option<String>,
    pub feedback: Option<String>,
    pub score: Option<Decimal>,
    pub status: String,
}

const INTERVIEW_SELECT: &str = r#"
select i.id, i.candidate_id, (c.first_name || ' ' || c.last_name) as candidate_name,
       i.requisition_id, i.round, i.scheduled_at, i.mode,
       (e.first_name || ' ' || e.last_name) as interviewer_name,
       i.feedback, i.score, i.status
from interviews i
join candidates c on c.id = i.candidate_id
left join employees e on e.id = i.interviewer_id
"#;

#[utoipa::path(get, path = "/api/recruiting/interviews", tag = "recruiting",
    responses((status = 200, body = Vec<Interview>)))]
pub async fn interviews_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<Interview>>> {
    let sql = format!("{INTERVIEW_SELECT} order by i.scheduled_at desc limit 200");
    let rows = sqlx::query_as::<_, Interview>(&sql).fetch_all(&state.db).await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct InterviewInput {
    pub candidate_id: Uuid,
    pub round: Option<i32>,
    pub scheduled_at: DateTime<Utc>,
    pub mode: Option<String>,
    pub interviewer_id: Option<Uuid>,
}

#[utoipa::path(post, path = "/api/recruiting/interviews", tag = "recruiting",
    request_body = InterviewInput, responses((status = 200, body = Interview)))]
pub async fn interviews_create(
    _user: AuthUser<RecruitingOffice>,
    State(state): State<AppState>,
    Json(input): Json<InterviewInput>,
) -> ApiResult<Json<Interview>> {
    let id: Uuid = sqlx::query_scalar(
        r#"insert into interviews (candidate_id, requisition_id, round, scheduled_at, mode, interviewer_id)
           select $1, c.requisition_id, coalesce($2,1), $3, coalesce($4,'virtual'), $5
           from candidates c where c.id = $1
           returning id"#,
    )
    .bind(input.candidate_id)
    .bind(input.round)
    .bind(input.scheduled_at)
    .bind(&input.mode)
    .bind(input.interviewer_id)
    .fetch_one(&state.db)
    .await?;
    let sql = format!("{INTERVIEW_SELECT} where i.id = $1");
    let row = sqlx::query_as::<_, Interview>(&sql)
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(row))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct FeedbackInput {
    pub feedback: String,
    pub score: Option<Decimal>,
    pub status: Option<String>,
}

#[utoipa::path(post, path = "/api/recruiting/interviews/{id}/feedback", tag = "recruiting",
    params(("id" = Uuid, Path)), request_body = FeedbackInput,
    responses((status = 200, body = Interview)))]
pub async fn interview_feedback(
    _user: AuthUser<RecruitingOffice>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<FeedbackInput>,
) -> ApiResult<Json<Interview>> {
    sqlx::query(
        "update interviews set feedback = $2, score = $3, status = coalesce($4,'completed') where id = $1",
    )
    .bind(id)
    .bind(&input.feedback)
    .bind(input.score)
    .bind(input.status)
    .execute(&state.db)
    .await?;
    let sql = format!("{INTERVIEW_SELECT} where i.id = $1");
    let row = sqlx::query_as::<_, Interview>(&sql)
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(row))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct Offer {
    pub id: Uuid,
    pub candidate_id: Uuid,
    pub candidate_name: String,
    pub requisition_id: Option<Uuid>,
    pub annual_ctc: Decimal,
    pub offer_date: NaiveDate,
    pub expires_on: Option<NaiveDate>,
    pub status: String,
    pub accepted_on: Option<NaiveDate>,
}

const OFFER_SELECT: &str = r#"
select o.id, o.candidate_id, (c.first_name || ' ' || c.last_name) as candidate_name,
       o.requisition_id, o.annual_ctc, o.offer_date, o.expires_on, o.status, o.accepted_on
from offers o join candidates c on c.id = o.candidate_id
"#;

#[utoipa::path(get, path = "/api/recruiting/offers", tag = "recruiting", responses((status = 200, body = Vec<Offer>)))]
pub async fn offers_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<Offer>>> {
    let sql = format!("{OFFER_SELECT} order by o.created_at desc limit 200");
    let rows = sqlx::query_as::<_, Offer>(&sql).fetch_all(&state.db).await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct OfferInput {
    pub candidate_id: Uuid,
    pub position_id: Option<Uuid>,
    pub annual_ctc: Decimal,
    pub expires_on: Option<NaiveDate>,
}

#[utoipa::path(post, path = "/api/recruiting/offers", tag = "recruiting",
    request_body = OfferInput, responses((status = 200, body = Offer)))]
pub async fn offers_create(
    user: AuthUser<RecruitingOffice>,
    State(state): State<AppState>,
    Json(input): Json<OfferInput>,
) -> ApiResult<Json<Offer>> {
    if input.annual_ctc <= Decimal::ZERO {
        return Err(ApiError::bad("annual_ctc must be positive"));
    }
    let id: Uuid = sqlx::query_scalar(
        r#"insert into offers (candidate_id, requisition_id, position_id, annual_ctc, expires_on, status)
           select $1, c.requisition_id, $2, $3, $4, 'released'
           from candidates c where c.id = $1
           returning id"#,
    )
    .bind(input.candidate_id)
    .bind(input.position_id)
    .bind(input.annual_ctc)
    .bind(input.expires_on)
    .fetch_one(&state.db)
    .await?;
    sqlx::query("update candidates set stage = 'offer' where id = $1")
        .bind(input.candidate_id)
        .execute(&state.db)
        .await?;
    let sql = format!("{OFFER_SELECT} where o.id = $1");
    let row = sqlx::query_as::<_, Offer>(&sql)
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    state
        .audit(Some(&user.session), "release", "offer", Some(id.to_string()), serde_json::json!({"ctc": input.annual_ctc}))
        .await;
    Ok(Json(row))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct OfferResponse {
    pub accept: bool,
}

#[utoipa::path(post, path = "/api/recruiting/offers/{id}/respond", tag = "recruiting",
    params(("id" = Uuid, Path)), request_body = OfferResponse,
    responses((status = 200, body = Offer)))]
pub async fn offer_respond(
    _user: AuthUser<RecruitingOffice>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<OfferResponse>,
) -> ApiResult<Json<Offer>> {
    let status = if input.accept { "accepted" } else { "declined" };
    let candidate_id: Option<Uuid> = sqlx::query_scalar(
        "update offers set status = $2, accepted_on = case when $3 then current_date else null end
         where id = $1 returning candidate_id",
    )
    .bind(id)
    .bind(status)
    .bind(input.accept)
    .fetch_optional(&state.db)
    .await?;
    if let Some(candidate_id) = candidate_id {
        let stage = if input.accept { "hired" } else { "rejected" };
        sqlx::query("update candidates set stage = $2 where id = $1")
            .bind(candidate_id)
            .bind(stage)
            .execute(&state.db)
            .await?;
    }
    let sql = format!("{OFFER_SELECT} where o.id = $1");
    let row = sqlx::query_as::<_, Offer>(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::not_found("offer"))?;
    Ok(Json(row))
}

// ---------------------------------------------------------------------------
// Candidate Gateway (public)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct PublicRequisition {
    pub req_no: String,
    pub title: String,
    pub department_name: Option<String>,
    pub location_name: Option<String>,
    pub openings: i32,
    pub job_description: Option<String>,
    pub opened_on: NaiveDate,
}

#[utoipa::path(get, path = "/api/careers/requisitions", tag = "careers",
    responses((status = 200, body = Vec<PublicRequisition>)))]
pub async fn careers_requisitions(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<PublicRequisition>>> {
    let rows = sqlx::query_as::<_, PublicRequisition>(
        r#"select r.req_no, r.title, d.name as department_name, l.name as location_name,
                  r.openings, r.job_description, r.opened_on
           from requisitions r
           left join departments d on d.id = r.department_id
           left join locations l on l.id = r.location_id
           where r.status = 'open' order by r.opened_on desc"#,
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PublicApplication {
    pub req_no: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub phone: Option<String>,
}

#[utoipa::path(post, path = "/api/careers/apply", tag = "careers",
    request_body = PublicApplication, responses((status = 200, description = "application received")))]
pub async fn careers_apply(
    State(state): State<AppState>,
    Json(input): Json<PublicApplication>,
) -> ApiResult<Json<serde_json::Value>> {
    if !input.email.contains('@') {
        return Err(ApiError::bad("a valid email is required"));
    }
    let requisition_id: Option<Uuid> = sqlx::query_scalar(
        "select id from requisitions where req_no = $1 and status = 'open'",
    )
    .bind(&input.req_no)
    .fetch_optional(&state.db)
    .await?;
    let requisition_id =
        requisition_id.ok_or_else(|| ApiError::not_found("open requisition with that req_no"))?;

    sqlx::query(
        r#"insert into candidates (first_name, last_name, email, phone, requisition_id, source)
           values ($1,$2,$3,$4,$5,'careers_portal')
           on conflict (email, requisition_id) do nothing"#,
    )
    .bind(&input.first_name)
    .bind(&input.last_name)
    .bind(&input.email)
    .bind(&input.phone)
    .bind(requisition_id)
    .execute(&state.db)
    .await?;

    Ok(Json(serde_json::json!({
        "status": "received",
        "message": "Thank you — the recruiting team will contact you if your profile matches."
    })))
}

pub fn _unused(_: Utc) {}
