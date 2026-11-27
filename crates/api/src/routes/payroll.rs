//! Time & Pay — compensation, payroll runs, payslips, reversals, adjustments,
//! payroll interface exports and statutory filings.

use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Datelike, NaiveDate, Utc};
use hrms_core::compliance::{
    epf_ecr_csv, esi_csv, form16_csv, pt_csv, retention_until, tds_24q_csv, EsiFilingRow,
    Form16Data, PfFilingRow, PtFilingRow, TdsQuarterlyRow,
};
use hrms_core::domain::{EmploymentType, Gender, PayPeriod, TaxRegime};
use hrms_core::india::engine::{PayslipComputation, PayrollInput};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::{AnyRole, AuthUser, PayrollOffice};
use crate::error::{ApiError, ApiResult};
use crate::pdf::{payslip_pdf, PdfLine, PayslipPdfData};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/payroll/structures", get(structures_list).post(structures_create))
        .route("/payroll/compensation", get(compensation_list).post(compensation_create))
        .route("/payroll/compensation/budgets", get(budgets_list).post(budgets_upsert))
        .route("/payroll/variable", get(variable_list).post(variable_create))
        .route("/payroll/runs", get(runs_list).post(runs_create))
        .route("/payroll/runs/{id}", get(runs_get))
        .route("/payroll/runs/{id}/payslips", get(run_payslips))
        .route("/payroll/runs/{id}/reverse", post(runs_reverse))
        .route("/payroll/runs/{id}/export", post(runs_export))
        .route("/payroll/payslips", get(payslips_list))
        .route("/payroll/payslips/{id}", get(payslips_get))
        .route("/payroll/payslips/{id}/pdf", get(payslips_pdf))
        .route("/payroll/adjustments", get(adjustments_list).post(adjustments_create))
        .route("/payroll/filings", get(filings_list).post(filings_generate))
        .route("/payroll/filings/form16", post(filings_form16))
}

// ---------------------------------------------------------------------------
// Compensation
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct SalaryStructure {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub effective_from: NaiveDate,
    pub basic_pct: Decimal,
    pub hra_pct: Decimal,
    pub conveyance_pct: Decimal,
    pub special_pct: Decimal,
    pub notes: Option<String>,
}

#[utoipa::path(get, path = "/api/payroll/structures", tag = "payroll", responses((status = 200, body = Vec<SalaryStructure>)))]
pub async fn structures_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<SalaryStructure>>> {
    let rows = sqlx::query_as::<_, SalaryStructure>(
        "select id, code, name, effective_from, basic_pct, hra_pct, conveyance_pct, special_pct, notes
         from salary_structures order by effective_from desc",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct StructureInput {
    pub code: String,
    pub name: String,
    pub effective_from: NaiveDate,
    pub basic_pct: Decimal,
    pub hra_pct: Decimal,
    pub conveyance_pct: Decimal,
    pub special_pct: Decimal,
    pub notes: Option<String>,
}

/// Create a salary structure. `basic_pct` must be at least 50 so the Code on
/// Wages 50% wage rule is respected.
#[utoipa::path(post, path = "/api/payroll/structures", tag = "payroll", request_body = StructureInput,
    responses((status = 200, body = SalaryStructure), (status = 400, description = "basic below 50%")))]
pub async fn structures_create(
    user: AuthUser<PayrollOffice>,
    State(state): State<AppState>,
    Json(input): Json<StructureInput>,
) -> ApiResult<Json<SalaryStructure>> {
    if input.basic_pct < Decimal::from(50) {
        return Err(ApiError::bad("basic_pct must be at least 50% of remuneration"));
    }
    let total = input.basic_pct + input.hra_pct + input.conveyance_pct + input.special_pct;
    if total != Decimal::from(100) {
        return Err(ApiError::bad("component percentages must add up to 100"));
    }
    let row = sqlx::query_as::<_, SalaryStructure>(
        "insert into salary_structures (code, name, effective_from, basic_pct, hra_pct, conveyance_pct, special_pct, notes)
         values ($1,$2,$3,$4,$5,$6,$7,$8)
         returning id, code, name, effective_from, basic_pct, hra_pct, conveyance_pct, special_pct, notes",
    )
    .bind(&input.code)
    .bind(&input.name)
    .bind(input.effective_from)
    .bind(input.basic_pct)
    .bind(input.hra_pct)
    .bind(input.conveyance_pct)
    .bind(input.special_pct)
    .bind(&input.notes)
    .fetch_one(&state.db)
    .await?;
    state
        .audit(Some(&user.session), "create", "salary_structure", Some(row.id.to_string()), serde_json::json!({"code": row.code}))
        .await;
    Ok(Json(row))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct CompensationRecord {
    pub id: Uuid,
    pub employee_id: Uuid,
    pub employee_name: String,
    pub structure_code: Option<String>,
    pub annual_remuneration: Decimal,
    pub basic: Decimal,
    pub hra: Decimal,
    pub conveyance: Decimal,
    pub special_allowance: Decimal,
    pub other_allowances: Decimal,
    pub effective_from: NaiveDate,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CompensationQuery {
    pub employee_id: Option<Uuid>,
}

#[utoipa::path(get, path = "/api/payroll/compensation", tag = "payroll",
    params(("employee_id" = Option<Uuid>, Query)),
    responses((status = 200, body = Vec<CompensationRecord>)))]
pub async fn compensation_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Query(query): Query<CompensationQuery>,
) -> ApiResult<Json<Vec<CompensationRecord>>> {
    let rows = sqlx::query_as::<_, CompensationRecord>(
        r#"select c.id, c.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  s.code as structure_code, c.annual_remuneration, c.basic, c.hra, c.conveyance,
                  c.special_allowance, c.other_allowances, c.effective_from
           from employee_compensation c
           join employees e on e.id = c.employee_id
           left join salary_structures s on s.id = c.structure_id
           where ($1::uuid is null or c.employee_id = $1)
           order by c.effective_from desc limit 500"#,
    )
    .bind(query.employee_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CompensationInput {
    pub employee_id: Uuid,
    pub structure_code: String,
    pub annual_remuneration: Decimal,
    pub effective_from: NaiveDate,
    pub other_allowances: Option<Decimal>,
}

/// Assign a salary structure and derive monthly components from the annual
/// remuneration. Monthly basic = annual * basic_pct / 12.
#[utoipa::path(post, path = "/api/payroll/compensation", tag = "payroll",
    request_body = CompensationInput, responses((status = 200, body = CompensationRecord)))]
pub async fn compensation_create(
    user: AuthUser<PayrollOffice>,
    State(state): State<AppState>,
    Json(input): Json<CompensationInput>,
) -> ApiResult<Json<CompensationRecord>> {
    if input.annual_remuneration <= Decimal::ZERO {
        return Err(ApiError::bad("annual_remuneration must be positive"));
    }
    let structure: Option<(Uuid, Decimal, Decimal, Decimal, Decimal)> = sqlx::query_as(
        "select id, basic_pct, hra_pct, conveyance_pct, special_pct from salary_structures where code = $1",
    )
    .bind(&input.structure_code)
    .fetch_optional(&state.db)
    .await?;
    let (structure_id, basic_pct, hra_pct, conveyance_pct, special_pct) =
        structure.ok_or_else(|| ApiError::bad("unknown structure_code"))?;

    let monthly = hrms_core::money::round2(input.annual_remuneration / Decimal::from(12));
    let basic = hrms_core::money::round2(monthly * basic_pct / Decimal::from(100));
    let hra = hrms_core::money::round2(monthly * hra_pct / Decimal::from(100));
    let conveyance = hrms_core::money::round2(monthly * conveyance_pct / Decimal::from(100));
    let special = monthly - basic - hra - conveyance;
    let other = input.other_allowances.unwrap_or(Decimal::ZERO);

    let id: Uuid = sqlx::query_scalar(
        r#"insert into employee_compensation
           (employee_id, structure_id, annual_remuneration, basic, hra, conveyance, special_allowance, other_allowances, effective_from)
           values ($1,$2,$3,$4,$5,$6,$7,$8,$9) returning id"#,
    )
    .bind(input.employee_id)
    .bind(structure_id)
    .bind(input.annual_remuneration)
    .bind(basic)
    .bind(hra)
    .bind(conveyance)
    .bind(special)
    .bind(other)
    .bind(input.effective_from)
    .fetch_one(&state.db)
    .await?;

    let row = sqlx::query_as::<_, CompensationRecord>(
        r#"select c.id, c.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  s.code as structure_code, c.annual_remuneration, c.basic, c.hra, c.conveyance,
                  c.special_allowance, c.other_allowances, c.effective_from
           from employee_compensation c
           join employees e on e.id = c.employee_id
           left join salary_structures s on s.id = c.structure_id
           where c.id = $1"#,
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    state
        .audit(Some(&user.session), "create", "compensation", Some(id.to_string()), serde_json::json!({"employee_id": input.employee_id}))
        .await;
    Ok(Json(row))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct Budget {
    pub id: Uuid,
    pub department_id: Uuid,
    pub department_name: String,
    pub fiscal_year: String,
    pub budget_amount: Decimal,
    pub committed_amount: Decimal,
}

#[utoipa::path(get, path = "/api/payroll/compensation/budgets", tag = "payroll", responses((status = 200, body = Vec<Budget>)))]
pub async fn budgets_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<Budget>>> {
    let rows = sqlx::query_as::<_, Budget>(
        r#"select b.id, b.department_id, d.name as department_name, b.fiscal_year,
                  b.budget_amount, b.committed_amount
           from compensation_budgets b join departments d on d.id = b.department_id
           order by b.fiscal_year desc, d.name"#,
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct BudgetInput {
    pub department_id: Uuid,
    pub fiscal_year: String,
    pub budget_amount: Decimal,
}

#[utoipa::path(post, path = "/api/payroll/compensation/budgets", tag = "payroll",
    request_body = BudgetInput, responses((status = 200, body = Budget)))]
pub async fn budgets_upsert(
    _user: AuthUser<PayrollOffice>,
    State(state): State<AppState>,
    Json(input): Json<BudgetInput>,
) -> ApiResult<Json<Budget>> {
    let id: Uuid = sqlx::query_scalar(
        r#"insert into compensation_budgets (department_id, fiscal_year, budget_amount)
           values ($1,$2,$3)
           on conflict (department_id, fiscal_year) do update set budget_amount = excluded.budget_amount
           returning id"#,
    )
    .bind(input.department_id)
    .bind(&input.fiscal_year)
    .bind(input.budget_amount)
    .fetch_one(&state.db)
    .await?;
    let row = sqlx::query_as::<_, Budget>(
        r#"select b.id, b.department_id, d.name as department_name, b.fiscal_year,
                  b.budget_amount, b.committed_amount
           from compensation_budgets b join departments d on d.id = b.department_id where b.id = $1"#,
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(row))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct VariablePay {
    pub id: Uuid,
    pub employee_id: Uuid,
    pub employee_name: String,
    pub period_year: i32,
    pub period_month: i32,
    pub component: String,
    pub amount: Decimal,
    pub reason: Option<String>,
    pub status: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct VariableQuery {
    pub year: Option<i32>,
    pub month: Option<i32>,
}

#[utoipa::path(get, path = "/api/payroll/variable", tag = "payroll",
    params(("year" = Option<i32>, Query), ("month" = Option<i32>, Query)),
    responses((status = 200, body = Vec<VariablePay>)))]
pub async fn variable_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Query(query): Query<VariableQuery>,
) -> ApiResult<Json<Vec<VariablePay>>> {
    let rows = sqlx::query_as::<_, VariablePay>(
        r#"select v.id, v.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  v.period_year, v.period_month, v.component, v.amount, v.reason, v.status
           from variable_pay v join employees e on e.id = v.employee_id
           where ($1::int is null or v.period_year = $1)
             and ($2::int is null or v.period_month = $2)
           order by v.period_year desc, v.period_month desc limit 500"#,
    )
    .bind(query.year)
    .bind(query.month)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct VariableInput {
    pub employee_id: Uuid,
    pub period_year: i32,
    pub period_month: i32,
    pub component: String,
    pub amount: Decimal,
    pub reason: Option<String>,
    pub approve: Option<bool>,
}

#[utoipa::path(post, path = "/api/payroll/variable", tag = "payroll",
    request_body = VariableInput, responses((status = 200, body = VariablePay)))]
pub async fn variable_create(
    user: AuthUser<PayrollOffice>,
    State(state): State<AppState>,
    Json(input): Json<VariableInput>,
) -> ApiResult<Json<VariablePay>> {
    if !(1..=12).contains(&input.period_month) {
        return Err(ApiError::bad("period_month must be 1..12"));
    }
    let status = if input.approve.unwrap_or(false) { "approved" } else { "pending" };
    let id: Uuid = sqlx::query_scalar(
        r#"insert into variable_pay (employee_id, period_year, period_month, component, amount, reason, status, approved_by)
           values ($1,$2,$3,$4,$5,$6,$7,$8) returning id"#,
    )
    .bind(input.employee_id)
    .bind(input.period_year)
    .bind(input.period_month)
    .bind(&input.component)
    .bind(input.amount)
    .bind(&input.reason)
    .bind(status)
    .bind(if status == "approved" { user.session.employee_id } else { None })
    .fetch_one(&state.db)
    .await?;
    let row = sqlx::query_as::<_, VariablePay>(
        r#"select v.id, v.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  v.period_year, v.period_month, v.component, v.amount, v.reason, v.status
           from variable_pay v join employees e on e.id = v.employee_id where v.id = $1"#,
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(row))
}

// ---------------------------------------------------------------------------
// Payroll runs
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct RunSummary {
    pub id: Uuid,
    pub period_year: i32,
    pub period_month: i32,
    pub status: String,
    pub employee_count: i32,
    pub total_gross: Decimal,
    pub total_net: Decimal,
    pub total_employer_cost: Decimal,
    pub total_tds: Decimal,
    pub run_at: Option<chrono::DateTime<Utc>>,
    pub notes: Option<String>,
}

#[utoipa::path(get, path = "/api/payroll/runs", tag = "payroll", responses((status = 200, body = Vec<RunSummary>)))]
pub async fn runs_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<RunSummary>>> {
    let rows = sqlx::query_as::<_, RunSummary>(
        "select id, period_year, period_month, status, employee_count, total_gross, total_net,
                total_employer_cost, total_tds, run_at, notes
         from payroll_runs order by period_year desc, period_month desc",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[utoipa::path(get, path = "/api/payroll/runs/{id}", tag = "payroll",
    params(("id" = Uuid, Path)), responses((status = 200, body = RunSummary)))]
pub async fn runs_get(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<RunSummary>> {
    let row = sqlx::query_as::<_, RunSummary>(
        "select id, period_year, period_month, status, employee_count, total_gross, total_net,
                total_employer_cost, total_tds, run_at, notes
         from payroll_runs where id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::not_found("payroll run"))?;
    Ok(Json(row))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct RunInput {
    pub period_year: i32,
    pub period_month: u32,
    pub employee_ids: Option<Vec<Uuid>>,
    pub notes: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RunResult {
    pub run: RunSummary,
    pub processed: usize,
    pub skipped: Vec<SkipReason>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SkipReason {
    pub employee_no: String,
    pub reason: String,
}

#[derive(Debug, FromRow)]
struct RunEmployee {
    employee_id: Uuid,
    emp_no: String,
    first_name: String,
    last_name: String,
    work_state: String,
    gender: String,
    disability: bool,
    employment_type: String,
    date_of_joining: NaiveDate,
    pf_exempt: bool,
    esi_exempt: bool,
    tax_regime: String,
    pan: Option<String>,
    uan: Option<String>,
    esic_ip: Option<String>,
    department_name: Option<String>,
    designation: Option<String>,
    basic: Option<Decimal>,
    hra: Option<Decimal>,
    conveyance: Option<Decimal>,
    special_allowance: Option<Decimal>,
    other_allowances: Option<Decimal>,
    annual_remuneration: Option<Decimal>,
}

fn completed_years(from: NaiveDate, to: NaiveDate) -> u32 {
    let mut years = to.year() - from.year();
    if (to.month(), to.day()) < (from.month(), from.day()) {
        years -= 1;
    }
    years.max(0) as u32
}

/// Run payroll for a period. Idempotent per period: a second run is refused
/// until the first is reversed.
#[utoipa::path(post, path = "/api/payroll/runs", tag = "payroll", request_body = RunInput,
    responses((status = 200, body = RunResult), (status = 409, description = "period already processed")))]
pub async fn runs_create(
    user: AuthUser<PayrollOffice>,
    State(state): State<AppState>,
    Json(input): Json<RunInput>,
) -> ApiResult<Json<RunResult>> {
    if !(1..=12).contains(&input.period_month) {
        return Err(ApiError::bad("period_month must be 1..12"));
    }
    let period = PayPeriod::new(input.period_year, input.period_month);
    let existing: Option<(Uuid, String)> =
        sqlx::query_as("select id, status from payroll_runs where period_year = $1 and period_month = $2")
            .bind(period.year)
            .bind(period.month as i32)
            .fetch_optional(&state.db)
            .await?;
    if let Some((id, status)) = existing {
        if status != "reversed" {
            return Err(ApiError::conflict(format!(
                "payroll for {} already exists ({status}); reverse run {id} before reprocessing",
                period.label()
            )));
        }
    }

    let employees = sqlx::query_as::<_, RunEmployee>(
        r#"select e.id as employee_id, e.emp_no, e.first_name, e.last_name, e.work_state, e.gender,
                  e.disability, e.employment_type, e.date_of_joining, e.pf_exempt, e.esi_exempt,
                  e.tax_regime, e.pan, e.uan, e.esic_ip, d.name as department_name,
                  p.title as designation, c.basic, c.hra, c.conveyance, c.special_allowance,
                  c.other_allowances, c.annual_remuneration
           from employees e
           left join departments d on d.id = e.department_id
           left join positions p on p.id = e.position_id
           left join lateral (
               select * from employee_compensation ec
               where ec.employee_id = e.id and ec.effective_from <= $2
               order by ec.effective_from desc limit 1
           ) c on true
           where e.status in ('active','probation','on_leave','notice')
             and ($1::uuid[] is null or e.id = any($1))
           order by e.emp_no"#,
    )
    .bind(input.employee_ids.clone())
    .bind(period.last_day())
    .fetch_all(&state.db)
    .await?;

    let run_id: Uuid = sqlx::query_scalar(
        r#"insert into payroll_runs (period_year, period_month, status, run_by, run_at, notes)
           values ($1,$2,'processing',$3,now(),$4)
           on conflict (period_year, period_month) do update
             set status = 'processing', run_by = excluded.run_by, run_at = now(), notes = excluded.notes
           returning id"#,
    )
    .bind(period.year)
    .bind(period.month as i32)
    .bind(user.session.user_id)
    .bind(&input.notes)
    .fetch_one(&state.db)
    .await?;

    let engine = hrms_core::india::engine::PayrollEngine::new((*state.config).clone());
    let mut processed = 0usize;
    let mut skipped: Vec<SkipReason> = Vec::new();

    for employee in &employees {
        let Some(basic) = employee.basic else {
            skipped.push(SkipReason {
                employee_no: employee.emp_no.clone(),
                reason: "no compensation record effective for this period".into(),
            });
            continue;
        };

        let unpaid_days: Decimal = sqlx::query_scalar(
            r#"select coalesce(sum(r.days),0) from leave_requests r
               join leave_types lt on lt.id = r.leave_type_id
               where r.employee_id = $1 and r.status = 'approved' and lt.paid = false
                 and r.from_date <= $3 and r.to_date >= $2"#,
        )
        .bind(employee.employee_id)
        .bind(period.first_day())
        .bind(period.last_day())
        .fetch_one(&state.db)
        .await?;

        let days_in_month = Decimal::from(period.days_in_month());
        let paid_days = (days_in_month - unpaid_days).max(Decimal::ZERO);

        let ot_hours: Decimal = sqlx::query_scalar(
            "select coalesce(sum(ot_hours),0) from timesheets
             where employee_id = $1 and status = 'approved' and work_date between $2 and $3",
        )
        .bind(employee.employee_id)
        .bind(period.first_day())
        .bind(period.last_day())
        .fetch_one(&state.db)
        .await?;

        #[derive(FromRow)]
        struct VariableRow {
            component: String,
            amount: Decimal,
        }
        let variables = sqlx::query_as::<_, VariableRow>(
            "select component, amount from variable_pay
             where employee_id = $1 and period_year = $2 and period_month = $3 and status = 'approved'",
        )
        .bind(employee.employee_id)
        .bind(period.year)
        .bind(period.month as i32)
        .fetch_all(&state.db)
        .await?;
        let mut incentive = Decimal::ZERO;
        let mut bonus = Decimal::ZERO;
        for v in variables {
            match v.component.as_str() {
                "bonus" => bonus += v.amount,
                "incentive" | "commission" | "quality_incentive" => incentive += v.amount,
                _ => incentive += v.amount,
            }
        }

        #[derive(FromRow)]
        struct AdjustmentRow {
            id: Uuid,
            component: String,
            amount: Decimal,
        }
        let adjustments = sqlx::query_as::<_, AdjustmentRow>(
            "select id, component, amount from payroll_adjustments
             where employee_id = $1 and period_year = $2 and period_month = $3 and status = 'pending'",
        )
        .bind(employee.employee_id)
        .bind(period.year)
        .bind(period.month as i32)
        .fetch_all(&state.db)
        .await?;
        let mut arrears = Decimal::ZERO;
        let mut reimbursements = Decimal::ZERO;
        let mut extra_earnings = Vec::new();
        let mut extra_deductions = Vec::new();
        for adj in &adjustments {
            match adj.component.as_str() {
                "arrears" => arrears += adj.amount,
                "reimbursement" => reimbursements += adj.amount,
                _ if adj.amount < Decimal::ZERO => extra_deductions.push(
                    hrms_core::india::engine::PayLineInput::new(
                        &adj.component,
                        &adj.component.replace('_', " "),
                        -adj.amount,
                    ),
                ),
                _ => extra_earnings.push(hrms_core::india::engine::PayLineInput::new(
                    &adj.component,
                    &adj.component.replace('_', " "),
                    adj.amount,
                )),
            }
        }

        let fy_start = period.fy_start_year() * 12 + 4;
        let this_period = period.year * 12 + period.month as i32;
        let ytd_tds: Decimal = sqlx::query_scalar(
            r#"select coalesce(sum(tds),0) from payslips
               where employee_id = $1 and not is_reversal and status <> 'reversed'
                 and (period_year * 12 + period_month) >= $2
                 and (period_year * 12 + period_month) < $3"#,
        )
        .bind(employee.employee_id)
        .bind(fy_start)
        .bind(this_period)
        .fetch_one(&state.db)
        .await?;

        let mut payroll_input = PayrollInput::new(employee.employee_id, period);
        payroll_input.paid_days = paid_days;
        payroll_input.basic = basic;
        payroll_input.hra = employee.hra.unwrap_or(Decimal::ZERO);
        payroll_input.conveyance = employee.conveyance.unwrap_or(Decimal::ZERO);
        payroll_input.special_allowance = employee.special_allowance.unwrap_or(Decimal::ZERO);
        payroll_input.other_allowances = employee.other_allowances.unwrap_or(Decimal::ZERO);
        payroll_input.incentive = incentive;
        payroll_input.bonus = bonus;
        payroll_input.arrears = arrears;
        payroll_input.overtime_hours = ot_hours;
        payroll_input.reimbursements = reimbursements;
        payroll_input.other_earnings = extra_earnings;
        payroll_input.other_deductions = extra_deductions;
        payroll_input.pf_applicable = !employee.pf_exempt;
        payroll_input.esi_applicable = !employee.esi_exempt;
        payroll_input.tax_regime =
            TaxRegime::from_str_opt(&employee.tax_regime).unwrap_or(TaxRegime::New);
        payroll_input.work_state = employee.work_state.clone();
        payroll_input.gender = match employee.gender.as_str() {
            "female" => Gender::Female,
            _ => Gender::Male,
        };
        payroll_input.disability = employee.disability;
        payroll_input.employment_type = EmploymentType::from_str_opt(&employee.employment_type)
            .unwrap_or(EmploymentType::FullTime);
        payroll_input.service_years = completed_years(employee.date_of_joining, period.last_day());
        payroll_input.annual_gross_projection = employee.annual_remuneration;
        payroll_input.ctc_annual = employee.annual_remuneration;
        payroll_input.ytd_tds = ytd_tds;
        payroll_input.pay_data_changed = !adjustments.is_empty() || arrears > Decimal::ZERO;

        let computation = match engine.compute(&payroll_input) {
            Ok(c) => c,
            Err(err) => {
                skipped.push(SkipReason {
                    employee_no: employee.emp_no.clone(),
                    reason: err.to_string(),
                });
                continue;
            }
        };

        insert_payslip(&state, run_id, &period, employee, &computation, false).await?;

        for adjustment in &adjustments {
            sqlx::query("update payroll_adjustments set status = 'applied' where id = $1")
                .bind(adjustment.id)
                .execute(&state.db)
                .await?;
        }
        processed += 1;
    }

    sqlx::query(
        r#"update payroll_runs r set
             status = 'processed',
             employee_count = coalesce((select count(*) from payslips p where p.run_id = r.id and not p.is_reversal), 0),
             total_gross = coalesce((select sum(gross) from payslips p where p.run_id = r.id and not p.is_reversal), 0),
             total_net = coalesce((select sum(net_pay) from payslips p where p.run_id = r.id and not p.is_reversal), 0),
             total_employer_cost = coalesce((select sum(employer_cost) from payslips p where p.run_id = r.id and not p.is_reversal), 0),
             total_tds = coalesce((select sum(tds) from payslips p where p.run_id = r.id and not p.is_reversal), 0)
           where r.id = $1"#,
    )
    .bind(run_id)
    .execute(&state.db)
    .await?;

    state
        .audit(
            Some(&user.session),
            "payroll_run",
            "payroll_run",
            Some(run_id.to_string()),
            serde_json::json!({"period": period.label(), "processed": processed}),
        )
        .await;

    let run = sqlx::query_as::<_, RunSummary>(
        "select id, period_year, period_month, status, employee_count, total_gross, total_net,
                total_employer_cost, total_tds, run_at, notes from payroll_runs where id = $1",
    )
    .bind(run_id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(RunResult {
        run,
        processed,
        skipped,
    }))
}

async fn insert_payslip(
    state: &AppState,
    run_id: Uuid,
    period: &PayPeriod,
    employee: &RunEmployee,
    computation: &PayslipComputation,
    is_reversal: bool,
) -> Result<Uuid, sqlx::Error> {
    let breakdown = serde_json::to_value(computation).unwrap_or(Value::Null);
    let payslip_id: Uuid = sqlx::query_scalar(
        r#"insert into payslips
           (run_id, employee_id, period_year, period_month, days_in_month, paid_days, lop_days,
            gross, total_deductions, net_pay, employer_cost, pf_wages, pf_employee,
            pf_employer_epf, pf_employer_eps, edli, pf_admin, esi_wages, esi_employee,
            esi_employer, pt_state, pt_amount, tds, gratuity_accrual, taxable_income_annual,
            tax_regime, is_reversal, status, breakdown)
           values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,
                   $21,$22,$23,$24,$25,$26,$27,'processed',$28)
           returning id"#,
    )
    .bind(run_id)
    .bind(employee.employee_id)
    .bind(period.year)
    .bind(period.month as i32)
    .bind(period.days_in_month() as i32)
    .bind(computation.paid_days)
    .bind(computation.lop_days)
    .bind(computation.gross_earnings)
    .bind(computation.total_deductions)
    .bind(computation.net_pay)
    .bind(computation.employer_cost)
    .bind(computation.pf.pf_wages)
    .bind(computation.pf.employee)
    .bind(computation.pf.employer_epf)
    .bind(computation.pf.employer_eps)
    .bind(computation.pf.edli)
    .bind(computation.pf.admin_charges)
    .bind(computation.esi.wages)
    .bind(computation.esi.employee)
    .bind(computation.esi.employer)
    .bind(&computation.professional_tax.state)
    .bind(computation.professional_tax.amount)
    .bind(computation.tds_monthly)
    .bind(computation.gratuity_accrual)
    .bind(computation.tds.taxable_income)
    .bind(computation.tds.regime.clone())
    .bind(is_reversal)
    .bind(breakdown)
    .fetch_one(&state.db)
    .await?;

    for message in &computation.messages {
        sqlx::query(
            "insert into payroll_messages (payslip_id, code, severity, message) values ($1,$2,$3,$4)",
        )
        .bind(payslip_id)
        .bind(&message.code)
        .bind(&message.severity)
        .bind(&message.text)
        .execute(&state.db)
        .await?;
    }

    let _ = employee.uan.as_ref();
    Ok(payslip_id)
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct PayslipSummary {
    pub id: Uuid,
    pub run_id: Uuid,
    pub employee_id: Uuid,
    pub employee_name: String,
    pub emp_no: String,
    pub period_year: i32,
    pub period_month: i32,
    pub paid_days: Decimal,
    pub lop_days: Decimal,
    pub gross: Decimal,
    pub total_deductions: Decimal,
    pub net_pay: Decimal,
    pub employer_cost: Decimal,
    pub tds: Decimal,
    pub pt_amount: Decimal,
    pub is_reversal: bool,
    pub status: String,
}

const PAYSLIP_SELECT: &str = r#"
select p.id, p.run_id, p.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
       e.emp_no, p.period_year, p.period_month, p.paid_days, p.lop_days, p.gross,
       p.total_deductions, p.net_pay, p.employer_cost, p.tds, p.pt_amount, p.is_reversal, p.status
from payslips p join employees e on e.id = p.employee_id
"#;

#[utoipa::path(get, path = "/api/payroll/runs/{id}/payslips", tag = "payroll",
    params(("id" = Uuid, Path)), responses((status = 200, body = Vec<PayslipSummary>)))]
pub async fn run_payslips(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<PayslipSummary>>> {
    let sql = format!("{PAYSLIP_SELECT} where p.run_id = $1 order by e.emp_no");
    let rows = sqlx::query_as::<_, PayslipSummary>(&sql)
        .bind(id)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PayslipQuery {
    pub employee_id: Option<Uuid>,
    pub year: Option<i32>,
    pub month: Option<i32>,
}

#[utoipa::path(get, path = "/api/payroll/payslips", tag = "payroll",
    params(("employee_id" = Option<Uuid>, Query), ("year" = Option<i32>, Query), ("month" = Option<i32>, Query)),
    responses((status = 200, body = Vec<PayslipSummary>)))]
pub async fn payslips_list(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Query(query): Query<PayslipQuery>,
) -> ApiResult<Json<Vec<PayslipSummary>>> {
    let employee_id = match user.session.role {
        hrms_core::Role::Agent => user.session.employee_id,
        _ => query.employee_id,
    };
    let sql = format!(
        "{PAYSLIP_SELECT}
         where ($1::uuid is null or p.employee_id = $1)
           and ($2::int is null or p.period_year = $2)
           and ($3::int is null or p.period_month = $3)
         order by p.period_year desc, p.period_month desc, e.emp_no limit 500"
    );
    let rows = sqlx::query_as::<_, PayslipSummary>(&sql)
        .bind(employee_id)
        .bind(query.year)
        .bind(query.month)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(rows))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct MessageRow {
    pub code: String,
    pub severity: String,
    pub message: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PayslipDetail {
    pub payslip: PayslipSummary,
    pub breakdown: Value,
    pub messages: Vec<MessageRow>,
}

async fn load_payslip(state: &AppState, id: Uuid) -> ApiResult<(PayslipSummary, Value, Vec<MessageRow>)> {
    let sql = format!("{PAYSLIP_SELECT} where p.id = $1");
    let row = sqlx::query_as::<_, PayslipSummary>(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::not_found("payslip"))?;
    let breakdown: Value = sqlx::query_scalar("select breakdown from payslips where id = $1")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    let messages = sqlx::query_as::<_, MessageRow>(
        "select code, severity, message from payroll_messages where payslip_id = $1 order by created_at",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok((row, breakdown, messages))
}

async fn authorize_payslip(user: &crate::auth::Session, slip: &PayslipSummary) -> ApiResult<()> {
    if user.role == hrms_core::Role::Agent && user.employee_id != Some(slip.employee_id) {
        return Err(ApiError::forbidden("agents may only view their own payslips"));
    }
    Ok(())
}

#[utoipa::path(get, path = "/api/payroll/payslips/{id}", tag = "payroll",
    params(("id" = Uuid, Path)), responses((status = 200, body = PayslipDetail)))]
pub async fn payslips_get(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<PayslipDetail>> {
    let (slip, breakdown, messages) = load_payslip(&state, id).await?;
    authorize_payslip(&user.session, &slip).await?;
    Ok(Json(PayslipDetail {
        payslip: slip,
        breakdown,
        messages,
    }))
}

/// Download the payslip PDF (generated on demand and cached in MinIO when
/// object storage is configured).
#[utoipa::path(get, path = "/api/payroll/payslips/{id}/pdf", tag = "payroll",
    params(("id" = Uuid, Path)), responses((status = 200, content_type = "application/pdf", body = Vec<u8>)))]
pub async fn payslips_pdf(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Response> {
    let (slip, breakdown, messages) = load_payslip(&state, id).await?;
    authorize_payslip(&user.session, &slip).await?;

    let computation: PayslipComputation = serde_json::from_value(breakdown)
        .map_err(|e| ApiError::internal(format!("payslip breakdown is corrupt: {e}")))?;
    let key = format!(
        "payslips/{}-{:02}/{}.pdf",
        slip.period_year, slip.period_month, slip.emp_no
    );
    let bytes = if state.storage.enabled() {
        match state.storage.get(&key).await {
            Ok(cached) => cached,
            Err(_) => {
                let rendered = render_payslip(&state, &slip, &computation, &messages)?;
                let _ = state
                    .storage
                    .put(&key, rendered.clone(), "application/pdf")
                    .await;
                rendered
            }
        }
    } else {
        render_payslip(&state, &slip, &computation, &messages)?
    };

    let mut response = bytes.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/pdf"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        header::HeaderValue::from_str(&format!(
            "inline; filename=\"{}-{:02}-{}.pdf\"",
            slip.emp_no, slip.period_year, slip.period_month
        ))
        .unwrap_or_else(|_| header::HeaderValue::from_static("inline")),
    );
    Ok(response)
}

fn render_payslip(
    state: &AppState,
    slip: &PayslipSummary,
    computation: &PayslipComputation,
    messages: &[MessageRow],
) -> ApiResult<Vec<u8>> {
    let data = PayslipPdfData {
        org_name: state.settings.org_name.clone(),
        org_tan: state.settings.org_tan.clone(),
        pf_code: state.settings.pf_establishment_code.clone(),
        esi_code: state.settings.esi_establishment_code.clone(),
        employee_name: slip.employee_name.clone(),
        employee_no: slip.emp_no.clone(),
        pan_last4: "0000".into(),
        uan: String::new(),
        designation: String::new(),
        department: String::new(),
        work_state: computation.professional_tax.state.clone(),
        period: format!("{:02}-{}", slip.period_month, slip.period_year),
        paid_days: format!("{}", slip.paid_days),
        lop_days: format!("{}", slip.lop_days),
        earnings: computation
            .earnings
            .iter()
            .map(|l| PdfLine {
                label: l.label.clone(),
                amount: l.amount,
                taxable: l.taxable,
            })
            .collect(),
        deductions: computation
            .deductions
            .iter()
            .map(|l| PdfLine {
                label: l.label.clone(),
                amount: l.amount,
                taxable: false,
            })
            .collect(),
        employer_contributions: computation
            .employer_contributions
            .iter()
            .map(|l| PdfLine {
                label: l.label.clone(),
                amount: l.amount,
                taxable: false,
            })
            .collect(),
        gross: computation.gross_earnings,
        total_deductions: computation.total_deductions,
        net_pay: computation.net_pay,
        employer_cost: computation.employer_cost,
        pf_wages: computation.pf.pf_wages,
        esi_wages: computation.esi.wages,
        pt: computation.professional_tax.amount,
        tds: computation.tds_monthly,
        taxable_income: computation.tds.taxable_income,
        tax_regime: computation.tds.regime.clone(),
        messages: messages.iter().map(|m| format!("{}: {}", m.code, m.message)).collect(),
    };
    payslip_pdf(&data).map_err(ApiError::internal)
}

/// Reverse a processed run: every payslip gets an equal and opposite reversal
/// row in the same run, and the run is marked `reversed`.
#[utoipa::path(post, path = "/api/payroll/runs/{id}/reverse", tag = "payroll",
    params(("id" = Uuid, Path)), responses((status = 200, body = RunSummary)))]
pub async fn runs_reverse(
    user: AuthUser<PayrollOffice>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<RunSummary>> {
    let (year, month, status): (i32, i32, String) =
        sqlx::query_as("select period_year, period_month, status from payroll_runs where id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| ApiError::not_found("payroll run"))?;
    if status == "reversed" {
        return Err(ApiError::bad("run is already reversed"));
    }
    let period = PayPeriod::new(year, month as u32);
    let engine = hrms_core::india::engine::PayrollEngine::new((*state.config).clone());

    #[derive(FromRow)]
    struct SlipRow {
        employee_id: Uuid,
        emp_no: String,
        first_name: String,
        last_name: String,
        work_state: String,
        gender: String,
        disability: bool,
        employment_type: String,
        date_of_joining: NaiveDate,
        pf_exempt: bool,
        esi_exempt: bool,
        tax_regime: String,
        uan: Option<String>,
        breakdown: Value,
    }
    let slips = sqlx::query_as::<_, SlipRow>(
        r#"select p.employee_id, e.emp_no, e.first_name, e.last_name, e.work_state, e.gender,
                  e.disability, e.employment_type, e.date_of_joining, e.pf_exempt, e.esi_exempt,
                  e.tax_regime, e.uan, p.breakdown
           from payslips p join employees e on e.id = p.employee_id
           where p.run_id = $1 and not p.is_reversal"#,
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    for slip in &slips {
        let original: PayslipComputation = serde_json::from_value(slip.breakdown.clone())
            .map_err(|e| ApiError::internal(format!("corrupt payslip: {e}")))?;
        let mut payroll_input = PayrollInput::new(slip.employee_id, period);
        payroll_input.paid_days = original.paid_days;
        payroll_input.basic = original
            .earnings
            .iter()
            .find(|l| l.code == "BASIC")
            .map(|l| l.amount)
            .unwrap_or(Decimal::ZERO);
        payroll_input.pf_applicable = !slip.pf_exempt;
        payroll_input.esi_applicable = !slip.esi_exempt;
        payroll_input.tax_regime =
            TaxRegime::from_str_opt(&slip.tax_regime).unwrap_or(TaxRegime::New);
        payroll_input.work_state = slip.work_state.clone();
        payroll_input.gender = match slip.gender.as_str() {
            "female" => Gender::Female,
            _ => Gender::Male,
        };
        payroll_input.disability = slip.disability;
        payroll_input.employment_type = EmploymentType::from_str_opt(&slip.employment_type)
            .unwrap_or(EmploymentType::FullTime);
        payroll_input.service_years = completed_years(slip.date_of_joining, period.last_day());

        let reversal = engine
            .reversal(&payroll_input)
            .map_err(|e| ApiError::bad(e.to_string()))?;
        let pseudo = RunEmployee {
            employee_id: slip.employee_id,
            emp_no: slip.emp_no.clone(),
            first_name: slip.first_name.clone(),
            last_name: slip.last_name.clone(),
            work_state: slip.work_state.clone(),
            gender: slip.gender.clone(),
            disability: slip.disability,
            employment_type: slip.employment_type.clone(),
            date_of_joining: slip.date_of_joining,
            pf_exempt: slip.pf_exempt,
            esi_exempt: slip.esi_exempt,
            tax_regime: slip.tax_regime.clone(),
            pan: None,
            uan: slip.uan.clone(),
            esic_ip: None,
            department_name: None,
            designation: None,
            basic: Some(Decimal::ZERO),
            hra: None,
            conveyance: None,
            special_allowance: None,
            other_allowances: None,
            annual_remuneration: None,
        };
        insert_payslip(&state, id, &period, &pseudo, &reversal, true).await?;
    }

    sqlx::query("update payslips set status = 'reversed' where run_id = $1 and not is_reversal")
        .bind(id)
        .execute(&state.db)
        .await?;
    sqlx::query("update payroll_runs set status = 'reversed' where id = $1")
        .bind(id)
        .execute(&state.db)
        .await?;
    state
        .audit(Some(&user.session), "reverse", "payroll_run", Some(id.to_string()), serde_json::json!({"payslips": slips.len()}))
        .await;

    let run = sqlx::query_as::<_, RunSummary>(
        "select id, period_year, period_month, status, employee_count, total_gross, total_net,
                total_employer_cost, total_tds, run_at, notes from payroll_runs where id = $1",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(run))
}

/// Export a processed run in the payroll interface layout (generic CSV that an
/// external payroll/GL system can consume).
#[utoipa::path(post, path = "/api/payroll/runs/{id}/export", tag = "payroll",
    params(("id" = Uuid, Path)),
    responses((status = 200, content_type = "text/csv", body = String)))]
pub async fn runs_export(
    user: AuthUser<PayrollOffice>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Response> {
    let sql = format!("{PAYSLIP_SELECT} where p.run_id = $1 and not p.is_reversal order by e.emp_no");
    let slips = sqlx::query_as::<_, PayslipSummary>(&sql)
        .bind(id)
        .fetch_all(&state.db)
        .await?;
    if slips.is_empty() {
        return Err(ApiError::not_found("payslips for this run"));
    }

    let mut csv = String::from(
        "EMP_NO,EMPLOYEE_NAME,PERIOD_YEAR,PERIOD_MONTH,PAID_DAYS,GROSS,DEDUCTIONS,NET,EMPLOYER_COST,TDS,PT\n",
    );
    for s in &slips {
        csv.push_str(&format!(
            "{},{},{},{},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2}\n",
            s.emp_no,
            s.employee_name.replace(',', " "),
            s.period_year,
            s.period_month,
            s.paid_days,
            s.gross,
            s.total_deductions,
            s.net_pay,
            s.employer_cost,
            s.tds,
            s.pt_amount
        ));
    }

    let key = format!("interface/payroll-run-{id}.csv");
    let mut stored = None;
    if state.storage.enabled() {
        stored = state
            .storage
            .put(&key, csv.clone().into_bytes(), "text/csv")
            .await
            .ok();
    }
    let object_key = stored.clone().unwrap_or(key);
    sqlx::query(
        "insert into payroll_interface_exports (run_id, target, format, object_key, row_count, status)
         values ($1,'generic','csv',$2,$3,$4)",
    )
    .bind(id)
    .bind(&object_key)
    .bind(slips.len() as i32)
    .bind(if stored.is_some() { "stored" } else { "inline" })
    .execute(&state.db)
    .await?;
    state
        .audit(Some(&user.session), "export", "payroll_run", Some(id.to_string()), serde_json::json!({"rows": slips.len()}))
        .await;

    Ok(csv_response(csv, &format!("payroll-run-{id}.csv")))
}

fn csv_response(body: String, filename: &str) -> Response {
    let mut response = body.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    if let Ok(value) = header::HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
        response.headers_mut().insert(header::CONTENT_DISPOSITION, value);
    }
    response
}

// ---------------------------------------------------------------------------
// Adjustments
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct Adjustment {
    pub id: Uuid,
    pub employee_id: Uuid,
    pub employee_name: String,
    pub period_year: i32,
    pub period_month: i32,
    pub component: String,
    pub amount: Decimal,
    pub reason: Option<String>,
    pub status: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AdjustmentQuery {
    pub employee_id: Option<Uuid>,
    pub year: Option<i32>,
    pub month: Option<i32>,
}

#[utoipa::path(get, path = "/api/payroll/adjustments", tag = "payroll",
    params(("employee_id" = Option<Uuid>, Query), ("year" = Option<i32>, Query), ("month" = Option<i32>, Query)),
    responses((status = 200, body = Vec<Adjustment>)))]
pub async fn adjustments_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Query(query): Query<AdjustmentQuery>,
) -> ApiResult<Json<Vec<Adjustment>>> {
    let rows = sqlx::query_as::<_, Adjustment>(
        r#"select a.id, a.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  a.period_year, a.period_month, a.component, a.amount, a.reason, a.status
           from payroll_adjustments a join employees e on e.id = a.employee_id
           where ($1::uuid is null or a.employee_id = $1)
             and ($2::int is null or a.period_year = $2)
             and ($3::int is null or a.period_month = $3)
           order by a.created_at desc limit 300"#,
    )
    .bind(query.employee_id)
    .bind(query.year)
    .bind(query.month)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AdjustmentInput {
    pub employee_id: Uuid,
    pub period_year: i32,
    pub period_month: i32,
    pub component: String,
    pub amount: Decimal,
    pub reason: Option<String>,
}

/// Queue a pay-data change for the next run (arrears, reimbursement, recovery).
#[utoipa::path(post, path = "/api/payroll/adjustments", tag = "payroll",
    request_body = AdjustmentInput, responses((status = 200, body = Adjustment)))]
pub async fn adjustments_create(
    user: AuthUser<PayrollOffice>,
    State(state): State<AppState>,
    Json(input): Json<AdjustmentInput>,
) -> ApiResult<Json<Adjustment>> {
    if !(1..=12).contains(&input.period_month) {
        return Err(ApiError::bad("period_month must be 1..12"));
    }
    if input.amount == Decimal::ZERO {
        return Err(ApiError::bad("amount must be non-zero"));
    }
    let id: Uuid = sqlx::query_scalar(
        r#"insert into payroll_adjustments (employee_id, period_year, period_month, component, amount, reason)
           values ($1,$2,$3,$4,$5,$6) returning id"#,
    )
    .bind(input.employee_id)
    .bind(input.period_year)
    .bind(input.period_month)
    .bind(&input.component)
    .bind(input.amount)
    .bind(&input.reason)
    .fetch_one(&state.db)
    .await?;
    let row = sqlx::query_as::<_, Adjustment>(
        r#"select a.id, a.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  a.period_year, a.period_month, a.component, a.amount, a.reason, a.status
           from payroll_adjustments a join employees e on e.id = a.employee_id where a.id = $1"#,
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(row))
}

// ---------------------------------------------------------------------------
// Statutory filings
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct Filing {
    pub id: Uuid,
    pub kind: String,
    pub period_year: i32,
    pub period_month: Option<i32>,
    pub quarter: Option<i32>,
    pub object_key: String,
    pub row_count: i32,
    pub total_amount: Decimal,
    pub created_at: chrono::DateTime<Utc>,
}

#[utoipa::path(get, path = "/api/payroll/filings", tag = "payroll", responses((status = 200, body = Vec<Filing>)))]
pub async fn filings_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<Filing>>> {
    let rows = sqlx::query_as::<_, Filing>(
        "select id, kind, period_year, period_month, quarter, object_key, row_count, total_amount, created_at
         from statutory_filings order by created_at desc limit 200",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct FilingInput {
    /// epf_ecr | esi | pt | tds_24q
    pub kind: String,
    pub year: i32,
    pub month: Option<i32>,
    pub quarter: Option<i32>,
}

#[derive(FromRow)]
struct FilingSlip {
    employee_id: Uuid,
    emp_no: String,
    employee_name: String,
    pan: Option<String>,
    uan: Option<String>,
    esic_ip: Option<String>,
    paid_days: Decimal,
    gross: Decimal,
    pf_wages: Decimal,
    pf_employee: Decimal,
    pf_employer_epf: Decimal,
    pf_employer_eps: Decimal,
    edli: Decimal,
    esi_wages: Decimal,
    esi_employee: Decimal,
    esi_employer: Decimal,
    pt_state: Option<String>,
    pt_amount: Decimal,
    tds: Decimal,
}

/// Generate a monthly/quarterly statutory file. Returns the CSV itself and
/// records the filing (object key points at MinIO when storage is enabled).
#[utoipa::path(post, path = "/api/payroll/filings", tag = "payroll", request_body = FilingInput,
    responses((status = 200, content_type = "text/csv", body = String), (status = 400, description = "unknown kind")))]
pub async fn filings_generate(
    user: AuthUser<PayrollOffice>,
    State(state): State<AppState>,
    Json(input): Json<FilingInput>,
) -> ApiResult<Response> {
    let slips = if input.kind == "tds_24q" {
        load_quarter_slips(&state, input.year, input.quarter.unwrap_or(1)).await?
    } else {
        let month = input
            .month
            .ok_or_else(|| ApiError::bad("month is required for monthly filings"))?;
        load_month_slips(&state, input.year, month).await?
    };
    if slips.is_empty() {
        return Err(ApiError::not_found("processed payslips for the selected period"));
    }

    let (body, total, filename) = match input.kind.as_str() {
        "epf_ecr" => {
            let period = PayPeriod::new(input.year, input.month.unwrap_or(1).clamp(1, 12) as u32);
            let rows: Vec<PfFilingRow> = slips
                .iter()
                .map(|s| PfFilingRow {
                    uan: s.uan.clone().unwrap_or_default(),
                    member_name: s.employee_name.clone(),
                    gross_wages: s.gross,
                    epf_wages: s.pf_wages,
                    eps_wages: state
                        .config
                        .eps_ceiling_for(period.last_day(), s.pf_wages),
                    edli_wages: s.pf_wages,
                    employee_share: s.pf_employee,
                    eps_contribution: s.pf_employer_eps,
                    epf_employer: s.pf_employer_epf,
                    ncp_days: (Decimal::from(period.days_in_month()) - s.paid_days).max(Decimal::ZERO).to_i32().unwrap_or(0),
                })
                .collect();
            let total: Decimal = rows
                .iter()
                .map(|r| r.employee_share + r.eps_contribution + r.epf_employer)
                .sum();
            (
                epf_ecr_csv(&rows),
                total,
                format!("epf-ecr-{}-{:02}.csv", input.year, input.month.unwrap_or(1)),
            )
        }
        "esi" => {
            let rows: Vec<EsiFilingRow> = slips
                .iter()
                .filter(|s| s.esi_wages > Decimal::ZERO)
                .map(|s| EsiFilingRow {
                    ip_number: s.esic_ip.clone().unwrap_or_else(|| s.emp_no.clone()),
                    ip_name: s.employee_name.clone(),
                    total_wages: s.esi_wages,
                    employee_contribution: s.esi_employee,
                    employer_contribution: s.esi_employer,
                    contribution_days: s.paid_days.to_i32().unwrap_or(0),
                })
                .collect();
            let total = rows.iter().map(|r| r.employee_contribution + r.employer_contribution).sum();
            (
                esi_csv(&rows),
                total,
                format!("esi-{}-{:02}.csv", input.year, input.month.unwrap_or(1)),
            )
        }
        "pt" => {
            let mut grouped: std::collections::BTreeMap<String, (i64, Decimal, Decimal)> =
                std::collections::BTreeMap::new();
            for s in &slips {
                let entry = grouped
                    .entry(s.pt_state.clone().unwrap_or_else(|| "NA".into()))
                    .or_insert((0, Decimal::ZERO, Decimal::ZERO));
                entry.0 += 1;
                entry.1 += s.gross;
                entry.2 += s.pt_amount;
            }
            let rows: Vec<PtFilingRow> = grouped
                .into_iter()
                .map(|(state_code, (count, wages, tax))| PtFilingRow {
                    state: state_code,
                    employee_count: count,
                    total_wages: wages,
                    tax_collected: tax,
                })
                .collect();
            let total = rows.iter().map(|r| r.tax_collected).sum();
            (
                pt_csv(&rows),
                total,
                format!("pt-{}-{:02}.csv", input.year, input.month.unwrap_or(1)),
            )
        }
        "tds_24q" => {
            let quarter = input.quarter.unwrap_or(1);
            let rows: Vec<TdsQuarterlyRow> = slips
                .iter()
                .map(|s| TdsQuarterlyRow {
                    pan: s.pan.clone().unwrap_or_default(),
                    employee_name: s.employee_name.clone(),
                    section: "192".into(),
                    amount_paid: s.gross,
                    tds_deducted: s.tds,
                    deduction_date: quarter_end_month(input.year, quarter)
                        .and_then(|m| {
                            NaiveDate::from_ymd_opt(input.year, m, 1)
                                .map(|d| d + chrono::Duration::days(30))
                        }),
                    challan_no: None,
                })
                .collect();
            let total = rows.iter().map(|r| r.tds_deducted).sum();
            (
                tds_24q_csv(&rows),
                total,
                format!("tds-24q-{}-Q{}.csv", input.year, quarter),
            )
        }
        other => return Err(ApiError::bad(format!("unknown filing kind `{other}`"))),
    };

    let key = format!("filings/{filename}");
    let stored = if state.storage.enabled() {
        state
            .storage
            .put(&key, body.clone().into_bytes(), "text/csv")
            .await
            .ok()
    } else {
        None
    };
    sqlx::query(
        r#"insert into statutory_filings
           (kind, period_year, period_month, quarter, object_key, row_count, total_amount, generated_by)
           values ($1,$2,$3,$4,$5,$6,$7,$8)"#,
    )
    .bind(&input.kind)
    .bind(input.year)
    .bind(input.month)
    .bind(input.quarter)
    .bind(stored.clone().unwrap_or(key))
    .bind(slips.len() as i32)
    .bind(total)
    .bind(user.session.employee_id)
    .execute(&state.db)
    .await?;
    state
        .audit(Some(&user.session), "generate", "statutory_filing", None, serde_json::json!({"kind": input.kind}))
        .await;

    Ok(csv_response(body, &filename))
}

fn quarter_end_month(year: i32, quarter: i32) -> Option<u32> {
    let _ = year;
    match quarter {
        1 => Some(6),
        2 => Some(9),
        3 => Some(12),
        4 => Some(3),
        _ => None,
    }
}

async fn load_month_slips(
    state: &AppState,
    year: i32,
    month: i32,
) -> ApiResult<Vec<FilingSlip>> {
    let rows = sqlx::query_as::<_, FilingSlip>(
        r#"select p.employee_id, e.emp_no, (e.first_name || ' ' || e.last_name) as employee_name,
                  e.pan, e.uan, e.esic_ip, p.paid_days, p.gross, p.pf_wages, p.pf_employee,
                  p.pf_employer_epf, p.pf_employer_eps, p.edli, p.esi_wages, p.esi_employee,
                  p.esi_employer, p.pt_state, p.pt_amount, p.tds
           from payslips p join employees e on e.id = p.employee_id
           where p.period_year = $1 and p.period_month = $2
             and not p.is_reversal and p.status <> 'reversed'
           order by e.emp_no"#,
    )
    .bind(year)
    .bind(month)
    .fetch_all(&state.db)
    .await?;
    Ok(rows)
}

async fn load_quarter_slips(
    state: &AppState,
    year: i32,
    quarter: i32,
) -> ApiResult<Vec<FilingSlip>> {
    let months: Vec<i32> = match quarter {
        1 => vec![4, 5, 6],
        2 => vec![7, 8, 9],
        3 => vec![10, 11, 12],
        4 => vec![1, 2, 3],
        _ => return Err(ApiError::bad("quarter must be 1..4")),
    };
    let mut out: std::collections::BTreeMap<Uuid, FilingSlip> = std::collections::BTreeMap::new();
    for month in months {
        let slips = load_month_slips(state, year, month).await?;
        for slip in slips {
            let entry = out.entry(slip.employee_id).or_insert(FilingSlip {
                employee_id: slip.employee_id,
                emp_no: slip.emp_no.clone(),
                employee_name: slip.employee_name.clone(),
                pan: slip.pan.clone(),
                uan: slip.uan.clone(),
                esic_ip: slip.esic_ip.clone(),
                paid_days: Decimal::ZERO,
                gross: Decimal::ZERO,
                pf_wages: Decimal::ZERO,
                pf_employee: Decimal::ZERO,
                pf_employer_epf: Decimal::ZERO,
                pf_employer_eps: Decimal::ZERO,
                edli: Decimal::ZERO,
                esi_wages: Decimal::ZERO,
                esi_employee: Decimal::ZERO,
                esi_employer: Decimal::ZERO,
                pt_state: slip.pt_state.clone(),
                pt_amount: Decimal::ZERO,
                tds: Decimal::ZERO,
            });
            entry.paid_days += slip.paid_days;
            entry.gross += slip.gross;
            entry.pf_wages += slip.pf_wages;
            entry.pf_employee += slip.pf_employee;
            entry.pf_employer_epf += slip.pf_employer_epf;
            entry.pf_employer_eps += slip.pf_employer_eps;
            entry.edli += slip.edli;
            entry.esi_wages += slip.esi_wages;
            entry.esi_employee += slip.esi_employee;
            entry.esi_employer += slip.esi_employer;
            entry.pt_amount += slip.pt_amount;
            entry.tds += slip.tds;
        }
    }
    Ok(out.into_values().collect())
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct Form16Input {
    pub employee_id: Uuid,
    pub financial_year_start: i32,
}

/// Form 16 Part B data extract for one employee and financial year.
#[utoipa::path(post, path = "/api/payroll/filings/form16", tag = "payroll",
    request_body = Form16Input, responses((status = 200, content_type = "text/csv", body = String)))]
pub async fn filings_form16(
    user: AuthUser<PayrollOffice>,
    State(state): State<AppState>,
    Json(input): Json<Form16Input>,
) -> ApiResult<Response> {
    #[derive(FromRow)]
    struct FormRow {
        employee_name: String,
        pan: Option<String>,
        gross: Decimal,
        tds: Decimal,
        taxable_income: Decimal,
        regime: String,
    }
    let month_index_start = input.financial_year_start * 12 + 4;
    let row = sqlx::query_as::<_, FormRow>(
        r#"select (e.first_name || ' ' || e.last_name) as employee_name, e.pan,
                  coalesce(sum(p.gross),0) as gross, coalesce(sum(p.tds),0) as tds,
                  coalesce(max(p.taxable_income_annual),0) as taxable_income,
                  coalesce(max(p.tax_regime),'new') as regime
           from payslips p join employees e on e.id = p.employee_id
           where p.employee_id = $1 and not p.is_reversal and p.status <> 'reversed'
             and (p.period_year * 12 + p.period_month) >= $2
             and (p.period_year * 12 + p.period_month) < $2 + 12
           group by e.first_name, e.last_name, e.pan"#,
    )
    .bind(input.employee_id)
    .bind(month_index_start)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::not_found("payslips for this employee/financial year"))?;

    let standard_deduction = if row.regime == "old" {
        Decimal::from(50_000)
    } else {
        Decimal::from(75_000)
    };
    let data = Form16Data {
        employee_name: row.employee_name,
        pan: row.pan.unwrap_or_default(),
        assessment_year: format!(
            "{}-{}",
            input.financial_year_start + 1,
            (input.financial_year_start + 2) % 100
        ),
        employer_name: state.settings.org_name.clone(),
        employer_tan: state.settings.org_tan.clone(),
        gross_salary: row.gross,
        standard_deduction,
        chapter_via_deductions: Decimal::ZERO,
        taxable_income: row.taxable_income,
        tax_payable: row.tds,
        tax_deducted: row.tds,
        regime: row.regime,
    };
    let body = form16_csv(&data);
    sqlx::query(
        r#"insert into statutory_filings (kind, period_year, quarter, object_key, row_count, total_amount, generated_by)
           values ('form16',$1,null,$2,1,$3,$4)"#,
    )
    .bind(input.financial_year_start)
    .bind(format!("filings/form16-{}-{}.csv", input.employee_id, input.financial_year_start))
    .bind(row.tds)
    .bind(user.session.employee_id)
    .execute(&state.db)
    .await?;
    Ok(csv_response(
        body,
        &format!("form16-{}-{}.csv", input.employee_id, input.financial_year_start),
    ))
}

/// Retention deadline exposure for an existing payslip period.
#[derive(Debug, Serialize, ToSchema)]
pub struct RetentionInfo {
    pub period: String,
    pub retention_until: NaiveDate,
}

pub fn retention_info(period: PayPeriod, years: u32) -> RetentionInfo {
    RetentionInfo {
        period: period.label(),
        retention_until: retention_until(period, years),
    }
}
