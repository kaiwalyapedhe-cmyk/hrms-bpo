# Sampark HRMS — on-premises HCM for contact centres

A self-hosted HRMS written in Rust, benchmarked against **Oracle PeopleSoft HCM 9.2**
module-by-module, with an **India statutory payroll engine**, a **real-time WebSocket KPI
dashboard** and a Docker Compose deployment that runs entirely inside your network.

| | |
|---|---|
| Backend | axum 0.8, sqlx 0.8 (PostgreSQL 16), tokio, JWT (jsonwebtoken), Argon2id, validator |
| Statutory core | Pure Rust crate `hrms-core` — EPF/EPS, ESI, TDS, PT, gratuity, compliance CSVs |
| Frontend | Leptos 0.6 CSR → WASM via Trunk, SVG charts, WebSocket with exponential-backoff reconnect |
| Jobs / queue | Redis 7 through `fred` (`txns`, CDR ingestion queue); payroll runs are synchronous + transactional |
| Object storage | MinIO via `aws-sdk-s3` (payslip PDFs, exports); optional — files stream inline when unset |
| PDF | `printpdf`, Indian digit grouping (₹12,34,567) |
| API docs | utoipa 5 + Swagger UI at `/swagger-ui`, OpenAPI at `/api-docs/openapi.json` |
| Containers | postgres, redis, minio (+ `mc` init), api, seed, web, nginx — cargo-chef multi-stage builds |

---

## 1. Architecture

```mermaid
flowchart LR
  subgraph Edge["Edge (host network)"]
    NG["nginx :80<br/>/ → SPA · /api → api · /ws → api"]
  end

  subgraph App["Application"]
    WEB["web container<br/>Trunk/WASM bundle + static nginx"]
    API["api container (axum :8080)<br/>REST /api · WS /ws/dashboard<br/>Swagger /swagger-ui"]
    SEED["seed container<br/>hrms-seed (one-shot)"]
  end

  subgraph Data["State"]
    PG[("PostgreSQL 16<br/>HR, time, payroll, QA, audit")]
    RD[("Redis 7<br/>job queue / CDR ingest")]
    MO[("MinIO<br/>PDFs, exports")]
  end

  subgraph Core["hrms-core (pure)"]
    CFG["india_payroll.toml"]
    ENG["PayrollEngine<br/>EPF · EPS · ESI · TDS · PT · Gratuity"]
    COMP["Compliance builders<br/>EPF/ESI/PT CSV · 24Q · Form 16"]
  end

  USER(["Browser<br/>Manager / TL / Agent / QA / Admin"]) --> NG
  NG --> WEB
  NG --> API
  API -- "broadcast (tokio::sync::broadcast)" --> API
  API --> PG
  API --> RD
  API --> MO
  API --> ENG
  ENG --> CFG
  API --> COMP
  SEED --> PG
  RD -- "worker: rpop" --> API
  API -. "CDR CSV / punch / QA / aux events<br/>metric_delta · agent_status frames" .-> NG
```

**Request path.** `nginx` terminates HTTP and forwards `/` to the static SPA, `/api/*` to axum,
`/ws/*` with the upgrade headers intact. The API authenticates every route with a JWT
(`Authorization: Bearer`), or `?token=` for the WebSocket, and enforces role bounds in
extractor types (`AuthUser<PeopleAdmin>`, `AuthUser<PayrollAdmin>`, `AuthUser<Supervisor>`, …).

**Real-time path.** `DashboardHub` owns a `tokio::sync::broadcast` channel plus the last-known
metric map and agent-status map. CDR ingestion, punches, QA submissions and aux changes call
`publish_metrics` / `publish_agent`; a 60-second ticker recomputes occupancy, shrinkage,
adherence and publishes only values that actually changed. Each connected socket receives a
scoped snapshot on connect (`full`, `workforce`, `quality_only`, `personal`) and delta frames
afterwards. The Leptos client patches signals per frame — no page reload.

---

## 2. Module mapping (PeopleSoft HCM 9.2 → this build)

| PeopleSoft HCM 9.2 | PeopleSoft objects | Implementation |
|---|---|---|
| Workforce Data Management | PERSON, JOB, POSITION, DEPT, LOCATION | `workforce.rs`; `departments`, `locations`, `positions`, `employees`, `job_history`, `global_assignments`, `onboarding_tasks` |
| Onboarding Activity Guides | Guided self-service checklists | `POST /api/employees/:id/onboarding`, `POST /api/onboarding/:id/complete` |
| Time and Labor | TL_RPTD_TIME, PAYABLE_TIME | `time.rs`; `punches`, `timesheets`+lines, break punches, `ot_requests` with supervisor approval, `shifts`, `shift_assignments`, `schedule` |
| Absence Management | GP_ABS_EVENT, accrual plans | `absence.rs`; `leave_types`, `leave_balances`, `leave_requests`, holidays, `POST /api/leave/accrual/run`, forecast endpoint |
| Payroll | PY_RUNCTL, PAYROLL_MESSAGE | `payroll.rs` + `hrms-core`; `payroll_runs`, `payslips` (breakdown JSONB), reversal run, `payroll_messages`, adjustments |
| Payroll Interface | PI_PAYMENT / interface staging | `GET /api/payroll/runs/:id/export` (CSV), `POST /api/payroll/filings` (EPF/ESI/PT/TDS CSVs) |
| Base Compensation | SALARY_STRUCTURE, COMP_RATEPROG | `payroll/structures`, `employee_compensation`, budgets per department |
| Variable Compensation | Variable pay / commissions | `payroll/variable` — bonus, incentive, commission, performance pay components |
| Benefits Administration | BEN_PROG_PLAN, dependent enrollment | `benefits.rs`; `benefit_plans`, `benefit_enrollments`, `benefit_dependents`, deduction linkage into payroll |
| Training / ELM | COURSE, CLASS_SESSION, certifications | `development.rs`; `courses`, `training_sessions`, `training_enrollments`, `certifications`, compliance flags |
| ePerformance | EP_GOALS, EP_REVIEW | goals with checkpoints and progress, review cycles, 360 reviewer rows |
| Profile Management | Competency library, ratings | `competencies`, `employee_competencies` with 1–5 ratings |
| Talent Acquisition Manager | HRS_REQ, HRS_APP, interview, offer | `recruiting.rs`; requisitions, pipeline stages, interviews + feedback, offers + candidate response |
| Candidate Gateway | External careers site | Public `GET /api/careers/requisitions`, `POST /api/careers/apply` |
| Employee Self-Service | MSS/ESS | `selfservice.rs` `GET /api/me/summary`, notifications, punches, leave, payslips; SPA "My workspace" |
| Manager Self-Service | MSS approvals | `GET /api/manager/overview`, timesheet/leave/OT decision endpoints, SPA "Team" |
| Call-centre extensions | n/a | QA forms + weighted scores + CSAT, aux codes + agent aux state + adherence, CDR CSV ingestion, KPI snapshots and live dashboard |

---

## 3. Repository tree

```
.
├── Cargo.toml                       # workspace: core, api, web, seed
├── Cargo.lock
├── docker-compose.yml               # postgres, redis, minio, api, seed, web, nginx
├── .env.example
├── config/
│   └── india_payroll.toml           # every statutory rate/ceiling/slab + source-year comments
├── crates/
│   ├── core/                        # pure statutory engine - no I/O
│   │   ├── src/{lib,money,domain,config,compliance}.rs
│   │   ├── src/india/{engine,epf,esi,tds,pt,gratuity}.rs
│   │   └── tests/payroll_statutory.rs
│   ├── api/                         # axum service
│   │   ├── src/{main,state,auth,error,realtime,metrics,jobs,queue,storage,pdf}.rs
│   │   └── src/routes/{mod,auth,workforce,time,absence,payroll,benefits,development,recruiting,quality,dashboard,selfservice,admin}.rs
│   ├── web/                         # Leptos CSR SPA
│   │   ├── index.html, styles.css
│   │   └── src/{lib,main,app,api,ws}.rs
│   └── seed/                        # hrms-seed demo data binary
├── migrations/                      # sqlx migrations 0001…0007
│   ├── 0001_core_hr.sql
│   ├── 0002_time_absence.sql
│   ├── 0003_payroll.sql
│   ├── 0004_benefits.sql
│   ├── 0005_development_recruiting.sql
│   ├── 0006_call_center.sql
│   └── 0007_platform.sql
├── docker/
│   ├── api.Dockerfile               # cargo-chef multi-stage → debian-slim
│   ├── web.Dockerfile               # trunk build → nginx static
│   ├── web-static.conf
│   └── nginx.conf                   # edge proxy (/api, /ws, /swagger-ui)
├── csv-templates/
│   ├── bulk_employees.csv
│   └── cdr_import.csv
└── scripts/
    ├── smoke.sh                     # endpoint + auth + WebSocket checklist
    ├── backup.sh                    # pg_dump + MinIO mirror, 7-year retention note
    └── restore.sh
```

---

## 4. Quick start (Docker Compose)

```bash
cp .env.example .env            # change JWT_SECRET, POSTGRES_PASSWORD, MINIO_ROOT_PASSWORD
docker compose up -d --build
docker compose ps               # api should be healthy, seed exited 0
```

| URL | What |
|---|---|
| http://localhost | SPA (login, dashboard, self-service, team, admin) |
| http://localhost/swagger-ui | Swagger UI for the whole API |
| http://localhost/api-docs/openapi.json | OpenAPI document |
| http://localhost:8080/health · `/health/ready` | Liveness and dependency readiness |
| http://localhost:9001 | MinIO console (`hrmsadmin` / value from `.env`) |

**Default credentials**

```
admin@hrms.local
ChangeMe#2026          ← SEED_PASSWORD in .env
role: super_admin
```

Only the SuperAdmin account is seeded; the 20 demo employees have no logins (they exist so
dashboard, payroll, QA and approvals have data). Create more users in
`POST /api/admin/users` and link an `employee_id` to give an agent self-service scope.

**Seed contents** (idempotent — exits early when `admin@hrms.local` exists): 4 locations,
departments, positions, 20 employees across KA/MH/WB/TN (1001–1020, including a fixed-term
agent), salary structures + compensation, leave types/balances/requests, shifts, punches,
timesheets, overtime, aux history, 72 CDRs, a QA form with weighted scores, 7 days of KPI
snapshots, benefit plans/enrollments/dependents, courses + certifications, goals/reviews,
competencies, requisitions/candidates/interviews/offers, plus a fully computed payroll run
for the previous month with payslips and payroll messages.

### Local development (no containers for the app)

```bash
# state only
docker compose up -d postgres redis minio minio-init

export DATABASE_URL=postgres://hrms:hrms_dev_password@localhost:5432/hrms
export JWT_SECRET=local-dev-secret-change-me
export REDIS_URL=redis://localhost:6379
export S3_ENDPOINT=http://localhost:9000 S3_BUCKET=hrms \
       S3_ACCESS_KEY=hrmsadmin S3_SECRET_KEY=hrms_dev_password

cargo run -p hrms-api           # migrates on boot, serves :8080
cargo run -p hrms-seed          # demo data (safe to re-run)
cargo test -p hrms-core         # 24 statutory tests

rustup target add wasm32-unknown-unknown
cargo install trunk --locked
cd crates/web && trunk serve --port 3001
# open http://127.0.0.1:3001/?api=http://localhost:8080/api
```

The `?api=` override is stored in `localStorage` (`hrms.api_base`), so the WASM bundle uses
the direct API origin during development without a rebuild.

---

## 5. India payroll reference

Everything below is data in [`config/india_payroll.toml`](config/india_payroll.toml); the file is
also embedded at compile time so the API boots with sane statutory behaviour if the mount is
missing. `HRMS_PAYROLL_CONFIG` points at a different file when needed; restart the API to adopt it.

### EPF / EPS (EPF & MP Act 1952, EPS 1995)

| Rule | Value | Config |
|---|---|---|
| Employee contribution | 12% of PF wages (basic + DA) | `epf.employee_rate` |
| Employer contribution | 12%, split 3.67% EPF + 8.33% EPS | `epf.employer_rate`, `epf.eps_rate` |
| Statutory wage ceiling | ₹15,000/month | `epf.wage_ceiling` |
| Announced hike to ₹25,000 | effective **17 Sep 2026**, **off by default** | `epf.ceiling_hike.enabled`, `.wage_ceiling`, `.effective_from` |
| EPS diversion cap | 8.33% of the applicable ceiling, remainder to EPF | `epf.eps_wage_ceiling` |
| EDLI + admin charges | 0.5% + 0.5% (employer) | `epf.edli_rate`, `epf.admin_charge_rate` |
| Apply ceiling to employee share | yes | `epf.apply_ceiling_to_employee` |

For an employee at ₹30,000 basic with the ₹15,000 ceiling on: EPF wages = ₹15,000 →
employee ₹1,800, employer EPS ₹1,250 (8.33% of ₹15,000), employer EPF ₹550 (12% − EPS).
Above the ceiling, EPS stays capped and the residual employer share goes to EPF. Enabling
the hike makes payslips dated on/after `effective_from` use ₹25,000.

### ESI (ESI Act 1948, rates w.e.f. 01-Jul-2019)

| Rule | Value | Config |
|---|---|---|
| Employee / employer | 0.75% / 3.25% | `esi.employee_rate`, `esi.employer_rate` |
| Wage ceiling | ₹21,000/month; ₹25,000 for persons with disability | `esi.wage_ceiling`, `esi.wage_ceiling_disability` |
| Rounding | each contribution rounded **up** to the next rupee | `esi.round_up_to_rupee` |

Above ₹21,000 ESI does not apply (the engine emits `ESI_CEILING_EXCEEDED` on the payslip).

### TDS — salary (s.115BAC new regime default, FY 2025-26 slabs)

| Rule | Value | Config |
|---|---|---|
| Default regime | new | `tds.default_regime` |
| New-regime slabs | 0–4L nil · 4–8L 5% · 8–12L 10% · 12–16L 15% · 16–20L 20% · 20–24L 25% · >24L 30% | `[[tds.new_regime_slabs]]` |
| Old-regime slabs | 0–2.5L nil · 2.5–5L 5% · 5–10L 20% · >10L 30% | `[[tds.old_regime_slabs]]` |
| Standard deduction | ₹75,000 new / ₹50,000 old | `tds.standard_deduction_new/_old` |
| 87A rebate | up to ₹60,000 when taxable income ≤ ₹12L → nil up to ₹12L taxable (₹12.75L salary) | `tds.rebate_new_income_limit`, `tds.rebate_new_max` |
| Old-regime 87A | ₹12,500 up to ₹5L | `tds.rebate_old_*` |
| Cess / surcharge | 4% cess, surcharge slabs, marginal relief on | `tds.cess_rate`, `[[tds.surcharge_slabs]]`, `tds.marginal_relief` |
| Chapter VI-A caps (old regime) | 80C ₹1.5L, 80D ₹25k, 80CCD(1B) ₹50k, home-loan interest ₹2L | `tds.cap_*` |

Regime election is per employee (`tax_regime` on the payroll input); switching emits
`TAX_REGIME_CHANGE`. TDS is projected on annual gross, reduced by exemptions/deductions, then
spread over the remaining months of the financial year.

### Professional tax (state statute, Article 276 cap ₹2,500/person/year)

Levied on the employee's **place of work**, not the employer's registered office.

| State | Cycle | Slabs (monthly taxable wage → amount) | February |
|---|---|---|---|
| Maharashtra (`MH`) | monthly | ≤7,500 nil · ≤10,000 ₹175 · above ₹200; women exempt ≤₹25,000 | +₹100 |
| Karnataka (`KA`) | monthly | ≤15,000 nil · ≤25,000 ₹200 · above ₹200 | +₹100 (₹300 total) |
| West Bengal (`WB`) | monthly | ≤10,000 nil · ≤15,000 ₹110 · ≤25,000 ₹130 · ≤40,000 ₹150 · above ₹200 | — |
| Tamil Nadu (`TN`) | half-yearly | ≤21,000 nil · ≤30,000 ₹135 · ≤45,000 ₹315 · ≤60,000 ₹690 · ≤75,000 ₹1,025 · above ₹1,250 | split over the six months of the cycle, trued up in the last month |

Unknown state → PT nil plus a `PT_STATE_NOT_CONFIGURED` payroll message (never a silent
over-deduction). No state can exceed the Article 276 cap.

### Gratuity (Payment of Gratuity Act 1972)

`(15 × last drawn wages × completed years) / 26`, wages = basic + DA, capped at ₹20,00,000.
Eligibility: **5 years** continuous service; waived for death/disablement; **1 year** for
fixed-term employees (2022 amendment). Service over 6 months in the final year rounds up.
The Code on Wages 50% wage rule is evaluated and reported as `WAGE_50PCT_RULE` when core
wages fall below half of total remuneration.

### Payroll message statuses

`PARTIAL_PERIOD`, `PAY_DATA_CHANGE`, `REVERSAL`, `ADJUSTMENT`, `NO_PAY`, `TAX_REGIME_CHANGE`,
`PF_CEILING_APPLIED`, `ESI_CEILING_EXCEEDED`, `PT_STATE_NOT_CONFIGURED`, `WAGE_50PCT_RULE`,
`NEGATIVE_NET`, `GRATUITY_NOT_ELIGIBLE`, `OVERTIME_PAID`, `BONUS_PAID` — stored per payslip in
`payroll_messages` with severity, so a payroll admin can see exactly why a number changed.

### Compliance outputs

| Endpoint | Output |
|---|---|
| `POST /api/payroll/filings` `{"kind":"epf_ecr","year":2026,"month":10}` | EPF ECR-style CSV: UAN, name, PF wages, employee 12%, EPS 8.33%, EPF 3.67%, EDLI, admin |
| `…"kind":"esi"` | ESI monthly contribution CSV |
| `…"kind":"pt"` | State-wise professional tax CSV |
| `…"kind":"tds_24q","quarter":2` | Quarterly 24Q-style deduction rows |
| `GET /api/payroll/filings` | Index of generated filings (stored in object storage) |
| `POST /api/payroll/filings/form16` `{"employee_id":"…","financial_year_start":2026}` | Form 16 Part B data (salary, exemptions, deductions, tax) |
| `GET /api/payroll/runs/:id/export` | Payroll interface CSV for an external GL/payroll system |

Filing days and the 7-year retention window are in `[compliance]`.

---

## 6. API surface

All REST paths are under `/api`; Swagger UI is `/swagger-ui` (the OpenAPI document lists the
same paths). Highlights:

| Area | Endpoints |
|---|---|
| Auth | `POST /api/auth/login`, `GET /api/auth/me`, `POST /api/auth/change-password`, `GET /api/auth/roles` |
| Core HR | `/api/departments`, `/api/locations`, `/api/positions`, `/api/employees` (+ `/:id`, `/:id/terminate`, `/:id/job-history`, `/:id/assignments`, `/employees/import`, `/team/members`) |
| Time | `/api/punches`, `/api/timesheets` (+ `/pending`, `/:id/approve`, `/:id/reject`), `/api/ot-requests` (+ `/:id/decide`), `/api/shifts`, `/api/shifts/assign`, `/api/schedule` |
| Absence | `/api/leave/types`, `/api/leave/balances`, `/api/leave/requests` (+ `/:id/decide`), `/api/leave/forecast`, `/api/leave/accrual/run`, `/api/holidays` |
| Payroll | `/api/payroll/structures`, `/compensation`, `/compensation/budgets`, `/variable`, `/runs` (+ `/:id`, `/:id/payslips`, `/:id/reverse`, `/:id/export`), `/payslips` (+ `/:id`, `/:id/pdf`), `/adjustments`, `/filings`, `/filings/form16` |
| Benefits | `/api/benefits/plans`, `/enrollments` (+ `/:id/terminate`), `/dependents` |
| Development | `/api/training/courses`, `/sessions`, `/enrollments` (+ `/:id/complete`), `/certifications`, `/performance/goals` (+ checkpoints/progress), `/performance/reviews`, `/profile/competencies`, `/profile/ratings` |
| Recruiting | `/api/recruiting/requisitions` (+ `/:id/status`), `/candidates` (+ `/:id/stage`), `/interviews` (+ `/:id/feedback`), `/offers` (+ `/:id/respond`); public `/api/careers/requisitions`, `/api/careers/apply` |
| Quality / call centre | `/api/quality/forms`, `/api/quality/scores`, `/api/aux/codes`, `/api/aux/status`, `/api/aux/history`, `/api/cdr/import` (multipart CSV), `/api/cdr` |
| Dashboard | `GET /api/dashboard/snapshot`, `/api/dashboard/history?hours=`, `/api/dashboard/team` |
| Self-service | `GET /api/me/summary`, `/api/me/notifications` (+ `/:id/read`), `GET /api/manager/overview` |
| Admin | `/api/admin/users` (+ `/:id`, `/:id/reset-password`), `/api/admin/audit`, `/api/admin/system` |

### WebSocket protocol

`ws(s)://host/ws/dashboard?token=<JWT>` — nginx upgrades it transparently.

```jsonc
// on connect (role-scoped)
{"type":"snapshot","scope":"full","metrics":{"clocked_in":14,"schedule_adherence":92.5,"aht_15m":341.2,"queue_wait":6,"csat_avg":4.3,"occupancy_pct":78.4,...},"agents":[...],"ts":"..."}

// only changed metrics after a punch / CDR / QA / aux event or the 60s tick
{"type":"metric_delta","metrics":{"aht_15m":338.7,"queue_wait":4},"ts":"..."}

{"type":"agent_status","agent":{"employee_id":"…","employee_name":"Arjun Sharma","aux_code":"ON_CALL","category":"busy","colour":"yellow","since":"…"}}

{"type":"personal_metrics","metrics":{"my_aht":312.0,"my_csat":4.5},"ts":"..."}  // Agent scope
```

Access: `SuperAdmin`/`Manager`/`TeamLead` → `full`; `QaAnalyst` → `quality_only` (CSAT/QA/AHT);
`Agent` → `personal` (own metrics and own agent row only); other HR roles → `workforce`
aggregates. This is enforced server-side when building the snapshot and when filtering frames.

The SPA reconnects with 1s → 2s → 4s … capped at 30s plus jitter, and shows
`Connecting / Live / Reconnecting` next to the title. Each frame patches only the affected
signal, so only those cards re-render.

---

## 7. RBAC

Ten roles (`hrms_core::Role`), enforced by typed extractors in axum and by nav gating in the SPA.

| Role | Sees |
|---|---|
| `super_admin` | everything, user administration, audit, system status |
| `hr_admin` | person/job, onboarding, positions, team members, absence administration |
| `payroll_admin` | compensation, payroll runs, reversals, payslips, filings, Form 16 |
| `benefits_admin` | plans, enrollments, dependents |
| `recruiter` | requisitions, candidates, interviews, offers, careers gateway |
| `manager` / `team_lead` | team overview, approvals (timesheets, leave, OT), full dashboard |
| `agent` | own punches, leave, payslips, personal dashboard metrics |
| `qa_analyst` | QA forms/scores, CSAT/AHT dashboard panels, aux visibility |
| `trainer` | courses, sessions, enrollments, certifications, competencies |

JWT: HS256, `sub`/`role`/`employee_id` claims, TTL from `JWT_TTL_MINUTES` (default 480).
Passwords: Argon2id. Every privileged mutation writes to `audit_log`.

---

## 8. Operations

### Seed and reset

```bash
docker compose run --rm seed                      # idempotent
docker compose down -v && docker compose up -d --build   # full reset (drops volumes)
```

### Backup, restore, retention

```bash
./scripts/backup.sh                  # backups/<timestamp>/{hrms.sql.gz,minio/,MANIFEST}
./scripts/restore.sh backups/<ts>    # asks for confirmation, then psql + mc mirror
```

Back up nightly and before every payroll close; payroll artefacts must be retained **7 years**
(`compliance.records_retention_years`). Store backups encrypted, off the application host.

### Smoke checklist

```bash
./scripts/smoke.sh                                # http://localhost
BASE=http://localhost:8080 ./scripts/smoke.sh     # direct to the API
```

Covers: `/health`, `/health/ready`, Swagger, login, authenticated reads (me, employees,
dashboard snapshot/history, payslips, leave balances, QA, admin system), anonymous 401, and a
WebSocket 101 upgrade. Manual follow-up: sign in, watch KPI cards update after
`POST /api/cdr/import`, punch in/out, and submit a QA score to see deltas arrive.

### Configuration reference

| Variable | Default | Purpose |
|---|---|---|
| `DATABASE_URL` | required | PostgreSQL connection string |
| `JWT_SECRET` | dev fallback | **set in production**; ≥32 chars |
| `JWT_TTL_MINUTES` | `480` | token lifetime |
| `BIND_ADDR` | `0.0.0.0:8080` | API listener |
| `WEB_ORIGIN` | `http://localhost` | CORS allow-list (comma separated, `*` allowed) |
| `ORG_NAME`, `ORG_TAN` | demo values | filing headers |
| `PF_ESTABLISHMENT_CODE`, `ESI_ESTABLISHMENT_CODE` | demo values | filing headers |
| `SEED_PASSWORD` | `ChangeMe#2026` | SuperAdmin seed password |
| `REDIS_URL` | empty | enables the CDR ingest queue; API runs without it |
| `S3_ENDPOINT`/`S3_BUCKET`/`S3_ACCESS_KEY`/`S3_SECRET_KEY`/`S3_REGION` | empty | enables object storage for PDFs/exports |
| `HRMS_PAYROLL_CONFIG` | `./config/india_payroll.toml` | statutory config path |
| `RUST_LOG` | `info,hrms_api=debug,sqlx=warn` | tracing filter |

### CSV import templates

* `csv-templates/bulk_employees.csv` → `POST /api/employees/import` (multipart field `file`).
  Required: `emp_no`, `date_of_joining` (`YYYY-MM-DD`). Optional: `first_name`, `last_name`,
  `email`, `phone`, `department_code`, `position_code`, `work_state` (default `KA`),
  `pan`, `uan`, `extension`, `employment_type`, `gender`.
* `csv-templates/cdr_import.csv` → `POST /api/cdr/import`. Required: `uniqueid`, `call_start`.
  Optional: `call_answer`, `call_end`, `direction`, `queue`, `agent_ext` (matched to
  `employees.extension`), `disposition`, `wait_seconds`, `talk_seconds`, `hold_seconds`,
  `wrap_seconds`, `abandoned`, `customer_number`. Rows are de-duplicated on `uniqueid`.

```bash
TOKEN=$(curl -s -X POST localhost/api/auth/login -H 'Content-Type: application/json' \
  -d '{"email":"admin@hrms.local","password":"ChangeMe#2026"}' | python3 -c 'import json,sys;print(json.load(sys.stdin)["token"])')
curl -s -X POST localhost/api/cdr/import -H "Authorization: Bearer $TOKEN" \
  -F file=@csv-templates/cdr_import.csv
curl -s -X POST localhost/api/employees/import -H "Authorization: Bearer $TOKEN" \
  -F file=@csv-templates/bulk_employees.csv
```

---

## 9. Tests

```bash
cargo test -p hrms-core
```

24 integration tests in `crates/core/tests/payroll_statutory.rs` cover the mandated cases and
more: EPF 12/3.67/8.33 split with the EPS cap, ESI rounding up above ₹21,000 and the disability
ceiling, PT by work state (KA/MH/WB/TN, February surcharge, TN half-yearly, Article 276 cap,
unconfigured state), TDS nil at ₹12L taxable / ₹12.75L salary under the new regime, marginal
relief above the rebate threshold, old-regime slabs, TDS spreading across remaining months,
partial-period proration, overtime double rate, reversal negation, gratuity eligibility
(5 years, fixed-term 1 year, death/disablement waiver, ₹20L cap), and the 50% wage rule.

---

## 10. Troubleshooting

| Symptom | Cause / fix |
|---|---|
| `api` restarts, logs `DATABASE_URL is required` | `.env` missing or not passed — `cp .env.example .env`, then `docker compose up -d` |
| `sqlx::migrate` checksum error | a migration file changed after being applied. Never edit an applied migration; add a new one, or `docker compose down -v` for a throwaway environment |
| SPA loads but every call 401 | JWT expired (`JWT_TTL_MINUTES`) or `JWT_SECRET` changed → sign out/in changes; changing the secret invalidates all sessions |
| Dashboard says *Reconnecting* forever | WebSocket blocked by a proxy. nginx must forward `Upgrade`/`Connection` (see `docker/nginx.conf`); corporate proxies often strip them |
| `/health/ready` says `redis_queue: disabled` | `REDIS_URL` unset or Redis unreachable — the API still works, CDR import runs inline |
| Payslip PDF 500 / storage warnings | MinIO unreachable; with `S3_ENDPOINT` unset the API streams generated PDFs inline instead of uploading |
| PT is zero for an employee | `work_state` is missing or not one of MH/KA/WB/TN; look for `PT_STATE_NOT_CONFIGURED` in payroll messages |
| EPF looks capped unexpectedly | `epf.ceiling_hike.enabled` still `false`; set it and restart, or check `PF_CEILING_APPLIED` on the payslip |
| `hrms-seed` exits non-zero on a rerun | it is idempotent by design; non-zero usually means the API never migrated or the DB is unreachable |
| WASM build: `uuid` randomness error | the web crate must not depend on `uuid` without the `js` feature; this build has no `uuid` dependency in `crates/web` |
| `trunk serve` cannot reach the API | open `http://127.0.0.1:3001/?api=http://localhost:8080/api`, or put it behind nginx |

---

## 11. Assumptions and limits

* **Statutory defaults are the FY 2025-26 / AY 2026-27 schedule as specified.** The EPF ceiling
  hike to ₹25,000 is implemented as a date-gated config flag and ships **disabled**, because it
  applies only once notified for an establishment. Update the TOML and restart — no rebuild.
* **TDS is a projection engine**, not a filing engine: annualised gross, exemptions and
  Chapter VI-A caps, surcharge with marginal relief, cess, rebate, spread across remaining
  months. Investment proofs are not collected; declare exemptions through the payroll input.
* **Payroll runs are synchronous and transactional.** Redis carries the CDR ingestion queue and
  is deliberately non-critical: without it, imports run inline.
* **The dashboard tick is 60 seconds** and history retains what the seed/metrics writer stored;
  no time-series database is used (PostgreSQL only).
* **FreeSWITCH/Asterisk integration is file-based** (`CDR CSV`), not ESL-live. Queue-wait
  counts therefore reflect ingested CDRs, as the brief allows.
* **One SuperAdmin login is seeded**; demo employees have no accounts. Add users via
  `POST /api/admin/users` to exercise agent/manager scopes.
* **Frontend is CSR (Leptos/WASM), not SSR/hydration.** It is served as static files behind
  nginx; server-side rendering was not necessary for an internal wallboard and would add a
  second server runtime.
* **Not executed in this environment:** Docker/Compose, a live PostgreSQL, Redis, MinIO, the
  seed binary, and `scripts/smoke.sh` could not be run in the build sandbox (no Docker daemon
  or database). Verification performed here: `cargo check --workspace` clean,
  `cargo check -p hrms-web --target wasm32-unknown-unknown` clean, and `cargo test -p hrms-core`
  → 24 passed. Images, migrations at runtime and the smoke script should be validated on the
  target host with `docker compose up -d --build && ./scripts/smoke.sh`.
