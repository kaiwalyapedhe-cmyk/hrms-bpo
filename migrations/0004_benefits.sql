-- 0004_benefits.sql — Benefits Administration (enrollment, dependents,
-- deduction linkage) with the linked payroll deduction component.

create table benefit_plans (
    id               uuid primary key default gen_random_uuid(),
    code             text not null unique,
    name             text not null,
    provider         text,
    category         text not null default 'health'
                     check (category in ('health','life','accident','retirement','wellness','other')),
    employee_cost    numeric(10,2) not null default 0,
    employer_cost    numeric(10,2) not null default 0,
    coverage_amount  numeric(14,2) not null default 0,
    payroll_component text not null default 'BENEFIT_DEDUCTION',
    description      text,
    active           boolean not null default true
);

create table dependents (
    id             uuid primary key default gen_random_uuid(),
    employee_id    uuid not null references employees(id) on delete cascade,
    name           text not null,
    relation       text not null,
    date_of_birth  date,
    aadhaar_last4  text,
    created_at     timestamptz not null default now()
);

create table benefit_enrollments (
    id                uuid primary key default gen_random_uuid(),
    employee_id       uuid not null references employees(id) on delete cascade,
    plan_id           uuid not null references benefit_plans(id),
    effective_from    date not null,
    effective_to      date,
    employee_cost     numeric(10,2) not null default 0,
    employer_cost     numeric(10,2) not null default 0,
    dependents_covered int not null default 0,
    status            text not null default 'active'
                      check (status in ('pending','active','terminated')),
    created_at        timestamptz not null default now(),
    unique (employee_id, plan_id, effective_from)
);
