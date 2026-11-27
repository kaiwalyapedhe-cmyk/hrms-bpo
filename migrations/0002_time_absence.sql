-- 0002_time_absence.sql — Time & Labor + Absence Management

create table shifts (
    id             uuid primary key default gen_random_uuid(),
    code           text not null unique,
    name           text not null,
    start_time     time not null,
    end_time       time not null,
    break_minutes  int not null default 45,
    grace_minutes  int not null default 5,
    is_night       boolean not null default false,
    active         boolean not null default true
);

create table shift_assignments (
    id           uuid primary key default gen_random_uuid(),
    employee_id  uuid not null references employees(id) on delete cascade,
    shift_id     uuid not null references shifts(id),
    work_date    date not null,
    created_at   timestamptz not null default now(),
    unique (employee_id, work_date)
);

create table punches (
    id           uuid primary key default gen_random_uuid(),
    employee_id  uuid not null references employees(id) on delete cascade,
    punch_time   timestamptz not null,
    punch_type   text not null check (punch_type in ('in','out','break_start','break_end')),
    source       text not null default 'web' check (source in ('web','biometric','mobile','import')),
    device_id    text,
    created_at   timestamptz not null default now()
);
create index punches_employee_time_idx on punches (employee_id, punch_time desc);

create table timesheets (
    id             uuid primary key default gen_random_uuid(),
    employee_id    uuid not null references employees(id) on delete cascade,
    work_date      date not null,
    shift_id       uuid references shifts(id),
    in_time        timestamptz,
    out_time       timestamptz,
    break_minutes  int not null default 0,
    regular_hours  numeric(6,2) not null default 0,
    ot_hours       numeric(6,2) not null default 0,
    status         text not null default 'submitted'
                   check (status in ('draft','submitted','approved','rejected','locked')),
    approved_by    uuid references employees(id),
    approved_at    timestamptz,
    notes          text,
    created_at     timestamptz not null default now(),
    unique (employee_id, work_date)
);
create index timesheets_date_idx on timesheets (work_date);

create table ot_requests (
    id           uuid primary key default gen_random_uuid(),
    employee_id  uuid not null references employees(id) on delete cascade,
    work_date    date not null,
    hours        numeric(5,2) not null,
    reason       text,
    status       text not null default 'pending'
                 check (status in ('pending','approved','rejected')),
    approver_id  uuid references employees(id),
    decided_at   timestamptz,
    created_at   timestamptz not null default now()
);

create table leave_types (
    id             uuid primary key default gen_random_uuid(),
    code           text not null unique,
    name           text not null,
    annual_quota   numeric(6,2) not null default 0,
    monthly_accrual numeric(5,2) not null default 0,
    carry_forward  boolean not null default false,
    encashable     boolean not null default false,
    paid           boolean not null default true,
    max_balance    numeric(6,2)
);

create table leave_balances (
    id             uuid primary key default gen_random_uuid(),
    employee_id    uuid not null references employees(id) on delete cascade,
    leave_type_id  uuid not null references leave_types(id),
    year           int not null,
    accrued        numeric(8,2) not null default 0,
    used           numeric(8,2) not null default 0,
    available      numeric(8,2) not null default 0,
    updated_at     timestamptz not null default now(),
    unique (employee_id, leave_type_id, year)
);

create table leave_requests (
    id             uuid primary key default gen_random_uuid(),
    employee_id    uuid not null references employees(id) on delete cascade,
    leave_type_id  uuid not null references leave_types(id),
    from_date      date not null,
    to_date        date not null,
    days           numeric(6,2) not null,
    reason         text,
    status         text not null default 'pending'
                   check (status in ('pending','approved','rejected','cancelled')),
    approver_id    uuid references employees(id),
    decided_at     timestamptz,
    created_at     timestamptz not null default now()
);
create index leave_requests_employee_idx on leave_requests (employee_id, status);

-- Accrual / usage ledger: lets Absence Management forecast future balances.
create table leave_accrual_ledger (
    id             uuid primary key default gen_random_uuid(),
    employee_id    uuid not null references employees(id) on delete cascade,
    leave_type_id  uuid not null references leave_types(id),
    period         date not null,
    delta          numeric(8,2) not null,
    reason         text not null,
    created_at     timestamptz not null default now()
);

-- Free-text holiday calendar used by leave-day counting and adherence.
create table holidays (
    id           uuid primary key default gen_random_uuid(),
    holiday_date date not null unique,
    name         text not null,
    location_id  uuid references locations(id)
);
