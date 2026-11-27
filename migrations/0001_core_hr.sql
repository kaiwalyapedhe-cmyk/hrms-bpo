-- 0001_core_hr.sql — Workforce Data Management (PeopleSoft "Core HR")
-- Person/Job records, position management, onboarding activity guides and
-- global assignments.

create table departments (
    id            uuid primary key default gen_random_uuid(),
    code          text not null unique,
    name          text not null,
    cost_center   text,
    created_at    timestamptz not null default now()
);

create table locations (
    id            uuid primary key default gen_random_uuid(),
    code          text not null unique,
    name          text not null,
    city          text not null,
    state_code    char(2) not null,
    country       char(2) not null default 'IN',
    timezone      text not null default 'Asia/Kolkata'
);

create table positions (
    id                      uuid primary key default gen_random_uuid(),
    code                    text not null unique,
    title                   text not null,
    department_id           uuid references departments(id),
    location_id             uuid references locations(id),
    reports_to_position_id  uuid references positions(id),
    fte                     numeric(4,2) not null default 1.00,
    budgeted_annual_salary  numeric(14,2) not null default 0,
    status                  text not null default 'approved'
                            check (status in ('draft','approved','frozen','closed')),
    created_at              timestamptz not null default now()
);

create table employees (
    id                    uuid primary key default gen_random_uuid(),
    emp_no                text not null unique,
    first_name            text not null,
    last_name             text not null,
    email                 text not null unique,
    phone                 text,
    date_of_birth         date,
    gender                text not null default 'other'
                          check (gender in ('male','female','other')),
    date_of_joining       date not null,
    date_of_exit          date,
    status                text not null default 'active'
                          check (status in ('active','probation','on_leave','notice','separated')),
    employment_type       text not null default 'full_time'
                          check (employment_type in ('full_time','part_time','fixed_term','contractor','intern')),
    position_id           uuid references positions(id),
    manager_id            uuid references employees(id),
    department_id         uuid references departments(id),
    location_id           uuid references locations(id),
    work_state            char(2) not null default 'KA',
    pan                   text,
    uan                   text,
    esic_ip               text,
    pf_exempt             boolean not null default false,
    esi_exempt            boolean not null default false,
    tax_regime            text not null default 'new' check (tax_regime in ('new','old')),
    disability            boolean not null default false,
    bank_account          text,
    ifsc                  text,
    highest_qualification text,
    created_at            timestamptz not null default now(),
    updated_at            timestamptz not null default now()
);

create index employees_department_idx on employees (department_id);
create index employees_manager_idx on employees (manager_id);
create index employees_status_idx on employees (status);

create table job_history (
    id              uuid primary key default gen_random_uuid(),
    employee_id     uuid not null references employees(id) on delete cascade,
    position_id     uuid references positions(id),
    effective_date  date not null,
    action          text not null,
    reason          text,
    annual_salary   numeric(14,2),
    created_at      timestamptz not null default now()
);

create table onboarding_tasks (
    id            uuid primary key default gen_random_uuid(),
    employee_id   uuid not null references employees(id) on delete cascade,
    title         text not null,
    category      text not null default 'documentation',
    due_date      date,
    status        text not null default 'pending'
                  check (status in ('pending','in_progress','completed','waived')),
    assigned_to   uuid references employees(id),
    completed_at  timestamptz,
    created_at    timestamptz not null default now()
);

create table global_assignments (
    id                uuid primary key default gen_random_uuid(),
    employee_id       uuid not null references employees(id) on delete cascade,
    host_country      char(2) not null,
    host_location     text not null,
    start_date        date not null,
    end_date          date,
    allowance_monthly numeric(14,2) not null default 0,
    status            text not null default 'planned'
                      check (status in ('planned','active','completed','cancelled')),
    created_at        timestamptz not null default now()
);

-- Documents live in MinIO; the row keeps the object key + provenance.
create table documents (
    id            uuid primary key default gen_random_uuid(),
    employee_id   uuid references employees(id) on delete cascade,
    kind          text not null,
    object_key    text not null,
    content_type  text not null default 'application/octet-stream',
    uploaded_by   uuid,
    created_at    timestamptz not null default now()
);
