//! HTTP surface. Every module below maps to a PeopleSoft HCM 9.2 functional
//! area; the router mounts them under `/api` and Swagger under `/swagger-ui`.

use axum::Router;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::state::AppState;

pub mod absence;
pub mod admin;
pub mod auth;
pub mod benefits;
pub mod development;
pub mod payroll;
pub mod recruiting;
pub mod selfservice;
pub mod time;
pub mod workforce;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Sampark HRMS API",
        version = "1.0.0",
        description = "On-premises HRMS for call centres: PeopleSoft HCM 9.2 module mapping \
                       and India statutory payroll (EPF/EPS, ESI, TDS, PT, gratuity)."
    ),
    paths(
        auth::auth_login,
        auth::auth_me,
        auth::auth_change_password,
        auth::auth_roles,
        workforce::departments_list,
        workforce::departments_create,
        workforce::employees_list,
        workforce::employees_get,
        workforce::employees_create,
        workforce::employees_update,
        workforce::employees_import,
        workforce::team_members,
        workforce::onboarding_list,
        time::punches_list,
        time::punches_create,
        time::timesheets_list,
        time::timesheets_submit,
        time::timesheets_pending,
        time::timesheets_approve,
        time::ot_list,
        time::ot_create,
        time::shifts_list,
        time::schedule_list,
        absence::leave_types_list,
        absence::leave_balances,
        absence::leave_requests_list,
        absence::leave_requests_create,
        absence::leave_decide,
        absence::leave_forecast,
        absence::leave_accrual_run,
        payroll::structures_list,
        payroll::compensation_list,
        payroll::compensation_create,
        payroll::runs_list,
        payroll::runs_create,
        payroll::runs_get,
        payroll::runs_reverse,
        payroll::run_payslips,
        payroll::payslips_list,
        payroll::payslips_get,
        payroll::adjustments_list,
        payroll::adjustments_create,
        payroll::filings_list,
        payroll::filings_generate,
        benefits::plans_list,
        benefits::enrollments_list,
        benefits::enrollments_create,
        benefits::dependents_list,
        development::courses_list,
        development::training_enrollments,
        development::goals_list,
        development::reviews_list,
        development::competencies_list,
        recruiting::requisitions_list,
        recruiting::candidates_list,
        recruiting::interviews_list,
        recruiting::offers_list,
        recruiting::careers_requisitions,
        recruiting::careers_apply,
        selfservice::me_summary,
        selfservice::me_notifications,
        selfservice::manager_overview,
        admin::users_list,
        admin::users_create,
        admin::audit_list,
        admin::system_status,
    ),
    tags(
        (name = "auth", description = "Login, JWT session and RBAC roles"),
        (name = "workforce", description = "Core HR: person/job, positions, onboarding, global assignments"),
        (name = "time", description = "Time & Labor: punches, timesheets, overtime, shifts"),
        (name = "absence", description = "Absence Management: leave, accruals, forecasting"),
        (name = "payroll", description = "Payroll: compensation, runs, payslips, reversal, filings"),
        (name = "benefits", description = "Benefits Administration"),
        (name = "development", description = "Training/ELM, ePerformance, Profile"),
        (name = "recruiting", description = "Talent Acquisition Manager and Candidate Gateway"),
        (name = "selfservice", description = "Employee and manager self-service"),
        (name = "careers", description = "Public candidate gateway"),
        (name = "admin", description = "Users, audit and runtime diagnostics")
    )
)]
pub struct ApiDoc;

pub fn api_routes() -> Router<AppState> {
    let api: Router<AppState> = Router::new()
        .merge(auth::routes())
        .merge(workforce::routes())
        .merge(time::routes())
        .merge(absence::routes())
        .merge(payroll::routes())
        .merge(benefits::routes())
        .merge(development::routes())
        .merge(recruiting::routes())
        .merge(selfservice::routes())
        .merge(admin::routes());
    // Everything documented in Swagger and proxied by nginx lives under /api.
    Router::new().nest("/api", api)
}

/// Swagger UI (state-free router, merged after the API is given its state).
pub fn swagger() -> Router {
    let mut router: Router = Router::new();
    router = router.merge(
        SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()),
    );
    router
}
