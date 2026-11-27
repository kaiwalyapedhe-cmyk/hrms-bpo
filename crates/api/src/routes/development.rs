//! Workforce Development — Training Administration / Enterprise Learning,
//! ePerformance (goals, checkpoints, 360 reviews) and Profile Management
//! (competency library with skill ratings).

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::{AnyRole, AuthUser, Supervisor, TrainingOffice};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/training/courses", get(courses_list).post(courses_create))
        .route("/training/sessions", get(sessions_list).post(sessions_create))
        .route("/training/enrollments", get(training_enrollments).post(training_enroll))
        .route("/training/enrollments/{id}/complete", post(training_complete))
        .route("/training/certifications", get(certifications_list).post(certifications_create))
        .route("/performance/goals", get(goals_list).post(goals_create))
        .route("/performance/goals/{id}/checkpoints", post(goal_checkpoint))
        .route("/performance/goals/{id}/progress", post(goal_progress))
        .route("/performance/reviews", get(reviews_list).post(reviews_create))
        .route("/profile/competencies", get(competencies_list).post(competencies_create))
        .route("/profile/ratings", get(ratings_list).post(ratings_upsert))
}

// ---------------------------------------------------------------------------
// Training / ELM
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct Course {
    pub id: Uuid,
    pub code: String,
    pub title: String,
    pub category: String,
    pub mandatory: bool,
    pub hours: Decimal,
    pub description: Option<String>,
    pub active: bool,
}

#[utoipa::path(get, path = "/api/training/courses", tag = "development", responses((status = 200, body = Vec<Course>)))]
pub async fn courses_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<Course>>> {
    let rows = sqlx::query_as::<_, Course>(
        "select id, code, title, category, mandatory, hours, description, active
         from courses order by category, title",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CourseInput {
    pub code: String,
    pub title: String,
    pub category: Option<String>,
    pub mandatory: Option<bool>,
    pub hours: Option<Decimal>,
    pub description: Option<String>,
}

#[utoipa::path(post, path = "/api/training/courses", tag = "development", request_body = CourseInput,
    responses((status = 200, body = Course)))]
pub async fn courses_create(
    _user: AuthUser<TrainingOffice>,
    State(state): State<AppState>,
    Json(input): Json<CourseInput>,
) -> ApiResult<Json<Course>> {
    let row = sqlx::query_as::<_, Course>(
        "insert into courses (code, title, category, mandatory, hours, description)
         values ($1,$2,coalesce($3,'compliance'),coalesce($4,false),coalesce($5,1),$6)
         returning id, code, title, category, mandatory, hours, description, active",
    )
    .bind(&input.code)
    .bind(&input.title)
    .bind(&input.category)
    .bind(input.mandatory)
    .bind(input.hours)
    .bind(&input.description)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(row))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct Session {
    pub id: Uuid,
    pub course_code: String,
    pub course_title: String,
    pub trainer_name: Option<String>,
    pub start_at: DateTime<Utc>,
    pub end_at: DateTime<Utc>,
    pub mode: String,
    pub location: Option<String>,
    pub capacity: i32,
    pub status: String,
}

#[utoipa::path(get, path = "/api/training/sessions", tag = "development", responses((status = 200, body = Vec<Session>)))]
pub async fn sessions_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<Session>>> {
    let rows = sqlx::query_as::<_, Session>(
        r#"select s.id, c.code as course_code, c.title as course_title,
                  (t.first_name || ' ' || t.last_name) as trainer_name,
                  s.start_at, s.end_at, s.mode, s.location, s.capacity, s.status
           from course_sessions s
           join courses c on c.id = s.course_id
           left join employees t on t.id = s.trainer_id
           order by s.start_at desc limit 200"#,
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SessionInput {
    pub course_code: String,
    pub trainer_id: Option<Uuid>,
    pub start_at: DateTime<Utc>,
    pub end_at: DateTime<Utc>,
    pub mode: Option<String>,
    pub location: Option<String>,
    pub capacity: Option<i32>,
}

#[utoipa::path(post, path = "/api/training/sessions", tag = "development", request_body = SessionInput,
    responses((status = 200, body = Session)))]
pub async fn sessions_create(
    _user: AuthUser<TrainingOffice>,
    State(state): State<AppState>,
    Json(input): Json<SessionInput>,
) -> ApiResult<Json<Session>> {
    if input.end_at <= input.start_at {
        return Err(ApiError::bad("end_at must be after start_at"));
    }
    let course_id: Uuid = sqlx::query_scalar("select id from courses where code = $1")
        .bind(&input.course_code)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::bad("unknown course_code"))?;
    let id: Uuid = sqlx::query_scalar(
        "insert into course_sessions (course_id, trainer_id, start_at, end_at, mode, location, capacity)
         values ($1,$2,$3,$4,coalesce($5,'virtual'),$6,coalesce($7,20)) returning id",
    )
    .bind(course_id)
    .bind(input.trainer_id)
    .bind(input.start_at)
    .bind(input.end_at)
    .bind(&input.mode)
    .bind(&input.location)
    .bind(input.capacity)
    .fetch_one(&state.db)
    .await?;
    let row = sqlx::query_as::<_, Session>(
        r#"select s.id, c.code as course_code, c.title as course_title,
                  (t.first_name || ' ' || t.last_name) as trainer_name,
                  s.start_at, s.end_at, s.mode, s.location, s.capacity, s.status
           from course_sessions s
           join courses c on c.id = s.course_id
           left join employees t on t.id = s.trainer_id
           where s.id = $1"#,
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(row))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct TrainingEnrollment {
    pub id: Uuid,
    pub employee_id: Uuid,
    pub employee_name: String,
    pub session_id: Uuid,
    pub course_title: String,
    pub status: String,
    pub score: Option<Decimal>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TrainingQuery {
    pub employee_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
}

#[utoipa::path(get, path = "/api/training/enrollments", tag = "development",
    params(("employee_id" = Option<Uuid>, Query), ("session_id" = Option<Uuid>, Query)),
    responses((status = 200, body = Vec<TrainingEnrollment>)))]
pub async fn training_enrollments(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Query(query): Query<TrainingQuery>,
) -> ApiResult<Json<Vec<TrainingEnrollment>>> {
    let employee_id = match user.session.role {
        hrms_core::Role::Agent => user.session.employee_id,
        _ => query.employee_id,
    };
    let rows = sqlx::query_as::<_, TrainingEnrollment>(
        r#"select te.id, te.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  te.session_id, c.title as course_title, te.status, te.score, te.completed_at
           from training_enrollments te
           join employees e on e.id = te.employee_id
           join course_sessions s on s.id = te.session_id
           join courses c on c.id = s.course_id
           where ($1::uuid is null or te.employee_id = $1)
             and ($2::uuid is null or te.session_id = $2)
           order by te.id desc limit 300"#,
    )
    .bind(employee_id)
    .bind(query.session_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TrainingEnrollInput {
    pub employee_id: Option<Uuid>,
    pub session_id: Uuid,
}

#[utoipa::path(post, path = "/api/training/enrollments", tag = "development",
    request_body = TrainingEnrollInput, responses((status = 200, body = TrainingEnrollment)))]
pub async fn training_enroll(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Json(input): Json<TrainingEnrollInput>,
) -> ApiResult<Json<TrainingEnrollment>> {
    let employee_id = input
        .employee_id
        .or(user.session.employee_id)
        .ok_or_else(|| ApiError::bad("employee_id is required"))?;
    let id: Uuid = sqlx::query_scalar(
        "insert into training_enrollments (employee_id, session_id) values ($1,$2)
         on conflict (employee_id, session_id) do update set status = 'enrolled' returning id",
    )
    .bind(employee_id)
    .bind(input.session_id)
    .fetch_one(&state.db)
    .await?;
    let row = sqlx::query_as::<_, TrainingEnrollment>(
        r#"select te.id, te.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  te.session_id, c.title as course_title, te.status, te.score, te.completed_at
           from training_enrollments te
           join employees e on e.id = te.employee_id
           join course_sessions s on s.id = te.session_id
           join courses c on c.id = s.course_id
           where te.id = $1"#,
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(row))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CompletionInput {
    pub score: Option<Decimal>,
}

/// Mark a training enrollment complete and, for mandatory compliance courses,
/// drop a certification record so audits can prove coverage.
#[utoipa::path(post, path = "/api/training/enrollments/{id}/complete", tag = "development",
    params(("id" = Uuid, Path)), request_body = CompletionInput,
    responses((status = 200, body = TrainingEnrollment)))]
pub async fn training_complete(
    _user: AuthUser<TrainingOffice>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<CompletionInput>,
) -> ApiResult<Json<TrainingEnrollment>> {
    #[derive(FromRow)]
    struct Row {
        employee_id: Uuid,
        course_id: Uuid,
        course_title: String,
        mandatory: bool,
        session_end: DateTime<Utc>,
    }
    let row = sqlx::query_as::<_, Row>(
        r#"select te.employee_id, c.id as course_id, c.title as course_title, c.mandatory,
                  s.end_at as session_end
           from training_enrollments te
           join course_sessions s on s.id = te.session_id
           join courses c on c.id = s.course_id
           where te.id = $1"#,
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::not_found("training enrollment"))?;

    sqlx::query(
        "update training_enrollments set status = 'completed', score = coalesce($2, score), completed_at = now() where id = $1",
    )
    .bind(id)
    .bind(input.score)
    .execute(&state.db)
    .await?;

    if row.mandatory {
        sqlx::query(
            "insert into certifications (employee_id, name, issuer, issued_on, expires_on)
             values ($1,$2,'Internal LMS',$3,$3 + interval '1 year')",
        )
        .bind(row.employee_id)
        .bind(&row.course_title)
        .bind(row.session_end.date_naive())
        .execute(&state.db)
        .await?;
    }

    let result = sqlx::query_as::<_, TrainingEnrollment>(
        r#"select te.id, te.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  te.session_id, c.title as course_title, te.status, te.score, te.completed_at
           from training_enrollments te
           join employees e on e.id = te.employee_id
           join course_sessions s on s.id = te.session_id
           join courses c on c.id = s.course_id
           where te.id = $1"#,
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(result))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct Certification {
    pub id: Uuid,
    pub employee_id: Uuid,
    pub employee_name: String,
    pub name: String,
    pub issuer: Option<String>,
    pub issued_on: NaiveDate,
    pub expires_on: Option<NaiveDate>,
    pub status: String,
}

#[utoipa::path(get, path = "/api/training/certifications", tag = "development",
    responses((status = 200, body = Vec<Certification>)))]
pub async fn certifications_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<Certification>>> {
    let rows = sqlx::query_as::<_, Certification>(
        r#"select c.id, c.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  c.name, c.issuer, c.issued_on, c.expires_on, c.status
           from certifications c join employees e on e.id = c.employee_id
           order by c.expires_on nulls last"#,
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CertificationInput {
    pub employee_id: Uuid,
    pub name: String,
    pub issuer: Option<String>,
    pub issued_on: NaiveDate,
    pub expires_on: Option<NaiveDate>,
}

#[utoipa::path(post, path = "/api/training/certifications", tag = "development",
    request_body = CertificationInput, responses((status = 200, body = Certification)))]
pub async fn certifications_create(
    _user: AuthUser<TrainingOffice>,
    State(state): State<AppState>,
    Json(input): Json<CertificationInput>,
) -> ApiResult<Json<Certification>> {
    let id: Uuid = sqlx::query_scalar(
        "insert into certifications (employee_id, name, issuer, issued_on, expires_on)
         values ($1,$2,$3,$4,$5) returning id",
    )
    .bind(input.employee_id)
    .bind(&input.name)
    .bind(&input.issuer)
    .bind(input.issued_on)
    .bind(input.expires_on)
    .fetch_one(&state.db)
    .await?;
    let row = sqlx::query_as::<_, Certification>(
        r#"select c.id, c.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  c.name, c.issuer, c.issued_on, c.expires_on, c.status
           from certifications c join employees e on e.id = c.employee_id where c.id = $1"#,
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(row))
}

// ---------------------------------------------------------------------------
// ePerformance
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct Goal {
    pub id: Uuid,
    pub employee_id: Uuid,
    pub employee_name: String,
    pub cycle: String,
    pub title: String,
    pub description: Option<String>,
    pub weight: Decimal,
    pub target: Option<String>,
    pub progress: i32,
    pub status: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct GoalQuery {
    pub employee_id: Option<Uuid>,
    pub cycle: Option<String>,
}

#[utoipa::path(get, path = "/api/performance/goals", tag = "development",
    params(("employee_id" = Option<Uuid>, Query), ("cycle" = Option<String>, Query)),
    responses((status = 200, body = Vec<Goal>)))]
pub async fn goals_list(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Query(query): Query<GoalQuery>,
) -> ApiResult<Json<Vec<Goal>>> {
    let employee_id = match user.session.role {
        hrms_core::Role::Agent => user.session.employee_id,
        _ => query.employee_id,
    };
    let rows = sqlx::query_as::<_, Goal>(
        r#"select g.id, g.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  g.cycle, g.title, g.description, g.weight, g.target, g.progress, g.status
           from goals g join employees e on e.id = g.employee_id
           where ($1::uuid is null or g.employee_id = $1)
             and ($2::text is null or g.cycle = $2)
           order by g.created_at desc"#,
    )
    .bind(employee_id)
    .bind(query.cycle)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct GoalInput {
    pub employee_id: Option<Uuid>,
    pub cycle: String,
    pub title: String,
    pub description: Option<String>,
    pub weight: Option<Decimal>,
    pub target: Option<String>,
}

#[utoipa::path(post, path = "/api/performance/goals", tag = "development", request_body = GoalInput,
    responses((status = 200, body = Goal)))]
pub async fn goals_create(
    user: AuthUser<Supervisor>,
    State(state): State<AppState>,
    Json(input): Json<GoalInput>,
) -> ApiResult<Json<Goal>> {
    let employee_id = input
        .employee_id
        .or(user.session.employee_id)
        .ok_or_else(|| ApiError::bad("employee_id is required"))?;
    let id: Uuid = sqlx::query_scalar(
        "insert into goals (employee_id, cycle, title, description, weight, target)
         values ($1,$2,$3,$4,coalesce($5,10),$6) returning id",
    )
    .bind(employee_id)
    .bind(&input.cycle)
    .bind(&input.title)
    .bind(&input.description)
    .bind(input.weight)
    .bind(&input.target)
    .fetch_one(&state.db)
    .await?;
    let row = load_goal(&state, id).await?;
    Ok(Json(row))
}

async fn load_goal(state: &AppState, id: Uuid) -> ApiResult<Goal> {
    let row = sqlx::query_as::<_, Goal>(
        r#"select g.id, g.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  g.cycle, g.title, g.description, g.weight, g.target, g.progress, g.status
           from goals g join employees e on e.id = g.employee_id where g.id = $1"#,
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::not_found("goal"))?;
    Ok(row)
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CheckpointInput {
    pub note: String,
    pub rating: Option<i32>,
}

#[utoipa::path(post, path = "/api/performance/goals/{id}/checkpoints", tag = "development",
    params(("id" = Uuid, Path)), request_body = CheckpointInput,
    responses((status = 200, description = "checkpoint recorded")))]
pub async fn goal_checkpoint(
    user: AuthUser<Supervisor>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<CheckpointInput>,
) -> ApiResult<Json<serde_json::Value>> {
    if let Some(rating) = input.rating {
        if !(1..=5).contains(&rating) {
            return Err(ApiError::bad("rating must be 1..5"));
        }
    }
    sqlx::query("insert into goal_checkpoints (goal_id, note, rating, created_by) values ($1,$2,$3,$4)")
        .bind(id)
        .bind(&input.note)
        .bind(input.rating)
        .bind(user.session.employee_id)
        .execute(&state.db)
        .await?;
    Ok(Json(serde_json::json!({ "status": "recorded" })))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ProgressInput {
    pub progress: i32,
}

#[utoipa::path(post, path = "/api/performance/goals/{id}/progress", tag = "development",
    params(("id" = Uuid, Path)), request_body = ProgressInput,
    responses((status = 200, body = Goal)))]
pub async fn goal_progress(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<ProgressInput>,
) -> ApiResult<Json<Goal>> {
    if !(0..=100).contains(&input.progress) {
        return Err(ApiError::bad("progress must be 0..100"));
    }
    sqlx::query("update goals set progress = $2, status = case when $2 = 100 then 'completed' else status end where id = $1")
        .bind(id)
        .bind(input.progress)
        .execute(&state.db)
        .await?;
    Ok(Json(load_goal(&state, id).await?))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct Review {
    pub id: Uuid,
    pub employee_id: Uuid,
    pub employee_name: String,
    pub reviewer_name: Option<String>,
    pub cycle: String,
    pub review_type: String,
    pub rating: Option<Decimal>,
    pub comments: Option<String>,
    pub status: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ReviewQuery {
    pub employee_id: Option<Uuid>,
    pub cycle: Option<String>,
}

const REVIEW_SELECT: &str = r#"
select r.id, r.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
       (m.first_name || ' ' || m.last_name) as reviewer_name,
       r.cycle, r.review_type, r.rating, r.comments, r.status
from performance_reviews r
join employees e on e.id = r.employee_id
left join employees m on m.id = r.reviewer_id
"#;

#[utoipa::path(get, path = "/api/performance/reviews", tag = "development",
    params(("employee_id" = Option<Uuid>, Query), ("cycle" = Option<String>, Query)),
    responses((status = 200, body = Vec<Review>)))]
pub async fn reviews_list(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Query(query): Query<ReviewQuery>,
) -> ApiResult<Json<Vec<Review>>> {
    let employee_id = match user.session.role {
        hrms_core::Role::Agent => user.session.employee_id,
        _ => query.employee_id,
    };
    let sql = format!(
        "{REVIEW_SELECT} where ($1::uuid is null or r.employee_id = $1)
         and ($2::text is null or r.cycle = $2) order by r.created_at desc"
    );
    let rows = sqlx::query_as::<_, Review>(&sql)
        .bind(employee_id)
        .bind(query.cycle)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ReviewInput {
    pub employee_id: Uuid,
    pub cycle: String,
    pub review_type: Option<String>,
    pub rating: Option<Decimal>,
    pub comments: Option<String>,
    pub submit: Option<bool>,
}

#[utoipa::path(post, path = "/api/performance/reviews", tag = "development",
    request_body = ReviewInput, responses((status = 200, body = Review)))]
pub async fn reviews_create(
    user: AuthUser<Supervisor>,
    State(state): State<AppState>,
    Json(input): Json<ReviewInput>,
) -> ApiResult<Json<Review>> {
    if let Some(rating) = input.rating {
        if rating < Decimal::ONE || rating > Decimal::from(5) {
            return Err(ApiError::bad("rating must be between 1 and 5"));
        }
    }
    let status = if input.submit.unwrap_or(false) { "submitted" } else { "draft" };
    let id: Uuid = sqlx::query_scalar(
        r#"insert into performance_reviews (employee_id, reviewer_id, cycle, review_type, rating, comments, status)
           values ($1,$2,$3,coalesce($4,'manager'),$5,$6,$7) returning id"#,
    )
    .bind(input.employee_id)
    .bind(user.session.employee_id)
    .bind(&input.cycle)
    .bind(&input.review_type)
    .bind(input.rating)
    .bind(&input.comments)
    .bind(status)
    .fetch_one(&state.db)
    .await?;
    let sql = format!("{REVIEW_SELECT} where r.id = $1");
    let row = sqlx::query_as::<_, Review>(&sql)
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(row))
}

// ---------------------------------------------------------------------------
// Profile / competencies
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct Competency {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub category: String,
    pub description: Option<String>,
}

#[utoipa::path(get, path = "/api/profile/competencies", tag = "development",
    responses((status = 200, body = Vec<Competency>)))]
pub async fn competencies_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<Competency>>> {
    let rows = sqlx::query_as::<_, Competency>(
        "select id, code, name, category, description from competencies order by category, name",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CompetencyInput {
    pub code: String,
    pub name: String,
    pub category: Option<String>,
    pub description: Option<String>,
}

#[utoipa::path(post, path = "/api/profile/competencies", tag = "development",
    request_body = CompetencyInput, responses((status = 200, body = Competency)))]
pub async fn competencies_create(
    _user: AuthUser<TrainingOffice>,
    State(state): State<AppState>,
    Json(input): Json<CompetencyInput>,
) -> ApiResult<Json<Competency>> {
    let row = sqlx::query_as::<_, Competency>(
        "insert into competencies (code, name, category, description)
         values ($1,$2,coalesce($3,'functional'),$4)
         returning id, code, name, category, description",
    )
    .bind(&input.code)
    .bind(&input.name)
    .bind(&input.category)
    .bind(&input.description)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(row))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct CompetencyRating {
    pub id: Uuid,
    pub employee_id: Uuid,
    pub employee_name: String,
    pub competency_code: String,
    pub competency_name: String,
    pub rating: i32,
    pub assessed_on: NaiveDate,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct RatingQuery {
    pub employee_id: Option<Uuid>,
}

#[utoipa::path(get, path = "/api/profile/ratings", tag = "development",
    params(("employee_id" = Option<Uuid>, Query)),
    responses((status = 200, body = Vec<CompetencyRating>)))]
pub async fn ratings_list(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Query(query): Query<RatingQuery>,
) -> ApiResult<Json<Vec<CompetencyRating>>> {
    let employee_id = match user.session.role {
        hrms_core::Role::Agent => user.session.employee_id,
        _ => query.employee_id,
    };
    let rows = sqlx::query_as::<_, CompetencyRating>(
        r#"select r.id, r.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  c.code as competency_code, c.name as competency_name, r.rating, r.assessed_on
           from competency_ratings r
           join employees e on e.id = r.employee_id
           join competencies c on c.id = r.competency_id
           where ($1::uuid is null or r.employee_id = $1)
           order by c.name"#,
    )
    .bind(employee_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct RatingInput {
    pub employee_id: Uuid,
    pub competency_code: String,
    pub rating: i32,
}

#[utoipa::path(post, path = "/api/profile/ratings", tag = "development",
    request_body = RatingInput, responses((status = 200, body = CompetencyRating)))]
pub async fn ratings_upsert(
    user: AuthUser<Supervisor>,
    State(state): State<AppState>,
    Json(input): Json<RatingInput>,
) -> ApiResult<Json<CompetencyRating>> {
    if !(1..=5).contains(&input.rating) {
        return Err(ApiError::bad("rating must be 1..5"));
    }
    let competency_id: Uuid = sqlx::query_scalar("select id from competencies where code = $1")
        .bind(&input.competency_code)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::bad("unknown competency_code"))?;
    let id: Uuid = sqlx::query_scalar(
        r#"insert into competency_ratings (employee_id, competency_id, rating, assessed_by)
           values ($1,$2,$3,$4)
           on conflict (employee_id, competency_id) do update
             set rating = excluded.rating, assessed_by = excluded.assessed_by, assessed_on = current_date
           returning id"#,
    )
    .bind(input.employee_id)
    .bind(competency_id)
    .bind(input.rating)
    .bind(user.session.employee_id)
    .fetch_one(&state.db)
    .await?;
    let row = sqlx::query_as::<_, CompetencyRating>(
        r#"select r.id, r.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  c.code as competency_code, c.name as competency_name, r.rating, r.assessed_on
           from competency_ratings r
           join employees e on e.id = r.employee_id
           join competencies c on c.id = r.competency_id
           where r.id = $1"#,
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(row))
}
