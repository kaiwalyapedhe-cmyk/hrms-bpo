-- 0003_payroll.sql — Time & Pay: payroll runs, payslips, messages,
-- adjustments, interface exports and statutory filings.

create table salary_structures (
    id             uuid primary key default gen_random_uuid(),
    code           text not null unique,
    name           text not null,
    effective_from date not null,
    basic_pct      numeric(5,2) not null default 50.00,
    hra_pct        numeric(5,2) not null default 20.00,
    conveyance_pct numeric(5,2) not null default 5.00,
    special_pct    numeric(5,2) not null default 25.00,
    notes          text,
    created_at     timestamptz not null default now(),
    check (basic_pct >= 50.00)
);

create table employee_compensation (
    id                uuid primary key default gen_random_uuid(),
    employee_id       uuid not null references employees(id) on delete cascade,
    structure_id      uuid references salary_structures(id),
    annual_remuneration numeric(14,2) not null,
    basic             numeric(12,2) not null,
    hra               numeric(12,2) not null default 0,
    conveyance        numeric(12,2) not null default 0,
    special_allowance numeric(12,2) not null default 0,
    other_allowances  numeric(12,2) not null default 0,
    effective_from    date not null,
    created_at        timestamptz not null default now()
);
create index comp_employee_idx on employee_compensation (employee_id, effective_from desc);

create table compensation_budgets (
    id              uuid primary key default gen_random_uuid(),
    department_id   uuid not null references departments(id),
    fiscal_year     text not null,
    budget_amount   numeric(16,2) not null default 0,
    committed_amount numeric(16,2) not null default 0,
    unique (department_id, fiscal_year)
);

create table variable_pay (
    id            uuid primary key default gen_random_uuid(),
    employee_id   uuid not null references employees(id) on delete cascade,
    period_year   int not null,
    period_month  int not null check (period_month between 1 and 12),
    component     text not null,
    amount        numeric(12,2) not null,
    reason        text,
    status        text not null default 'pending'
                  check (status in ('pending','approved','paid','cancelled')),
    approved_by   uuid references employees(id),
    created_at    timestamptz not null default now()
);

create table payroll_runs (
    id             uuid primary key default gen_random_uuid(),
    period_year    int not null,
    period_month   int not null check (period_month between 1 and 12),
    status         text not null default 'draft'
                   check (status in ('draft','processing','processed','posted','reversed','closed')),
    run_by         uuid,
    run_at         timestamptz,
    employee_count int not null default 0,
    total_gross    numeric(16,2) not null default 0,
    total_net      numeric(16,2) not null default 0,
    total_employer_cost numeric(16,2) not null default 0,
    total_tds      numeric(16,2) not null default 0,
    notes          text,
    created_at     timestamptz not null default now(),
    unique (period_year, period_month)
);

create table payslips (
    id                  uuid primary key default gen_random_uuid(),
    run_id              uuid not null references payroll_runs(id) on delete cascade,
    employee_id         uuid not null references employees(id),
    period_year         int not null,
    period_month        int not null,
    days_in_month       int not null,
    paid_days           numeric(5,2) not null,
    lop_days            numeric(5,2) not null default 0,
    gross               numeric(14,2) not null,
    total_deductions    numeric(14,2) not null,
    net_pay             numeric(14,2) not null,
    employer_cost       numeric(14,2) not null,
    pf_wages            numeric(12,2) not null default 0,
    pf_employee         numeric(12,2) not null default 0,
    pf_employer_epf     numeric(12,2) not null default 0,
    pf_employer_eps     numeric(12,2) not null default 0,
    edli                numeric(12,2) not null default 0,
    pf_admin            numeric(12,2) not null default 0,
    esi_wages           numeric(12,2) not null default 0,
    esi_employee        numeric(12,2) not null default 0,
    esi_employer        numeric(12,2) not null default 0,
    pt_state            char(2),
    pt_amount           numeric(10,2) not null default 0,
    tds                 numeric(12,2) not null default 0,
    gratuity_accrual    numeric(12,2) not null default 0,
    taxable_income_annual numeric(14,2) not null default 0,
    tax_regime          text not null default 'new',
    is_reversal         boolean not null default false,
    status              text not null default 'processed'
                        check (status in ('processed','held','reversed','paid')),
    breakdown           jsonb not null default '{}'::jsonb,
    created_at          timestamptz not null default now(),
    unique (run_id, employee_id, is_reversal)
);
create index payslips_employee_period_idx on payslips (employee_id, period_year desc, period_month desc);

-- PeopleSoft-style payroll message statuses (Partial Period, Pay Data Change,
-- Reversal, ...) persisted per payslip for audit.
create table payroll_messages (
    id          uuid primary key default gen_random_uuid(),
    payslip_id  uuid not null references payslips(id) on delete cascade,
    code        text not null,
    severity    text not null default 'info',
    message     text not null,
    created_at  timestamptz not null default now()
);

create table payroll_adjustments (
    id            uuid primary key default gen_random_uuid(),
    employee_id   uuid not null references employees(id) on delete cascade,
    period_year   int not null,
    period_month  int not null,
    component     text not null,
    amount        numeric(12,2) not null,
    reason        text,
    status        text not null default 'pending'
                  check (status in ('pending','applied','cancelled')),
    created_at    timestamptz not null default now()
);

create table payroll_interface_exports (
    id           uuid primary key default gen_random_uuid(),
    run_id       uuid not null references payroll_runs(id) on delete cascade,
    target       text not null,
    format       text not null default 'csv',
    object_key   text not null,
    row_count    int not null default 0,
    status       text not null default 'generated',
    created_at   timestamptz not null default now()
);

create table statutory_filings (
    id            uuid primary key default gen_random_uuid(),
    kind          text not null check (kind in ('epf_ecr','esi','pt','tds_24q','form16')),
    period_year   int not null,
    period_month  int,
    quarter       int,
    object_key    text not null,
    row_count     int not null default 0,
    total_amount  numeric(16,2) not null default 0,
    generated_by  uuid,
    created_at    timestamptz not null default now()
);
