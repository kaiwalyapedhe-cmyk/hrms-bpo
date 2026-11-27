-- 0006_call_center.sql — Call-centre extensions: aux codes / adherence,
-- FreeSWITCH-Asterisk CDR ingestion, QA scorecards and KPI snapshots.

create table aux_codes (
    id         uuid primary key default gen_random_uuid(),
    code       text not null unique,
    name       text not null,
    category   text not null check (category in ('available','on_call','break','offline')),
    is_paid    boolean not null default true,
    colour     text not null default 'gray',
    active     boolean not null default true
);

create table agent_aux_state (
    id           uuid primary key default gen_random_uuid(),
    employee_id  uuid not null references employees(id) on delete cascade,
    aux_code_id  uuid not null references aux_codes(id),
    started_at   timestamptz not null default now(),
    ended_at     timestamptz,
    source       text not null default 'web'
);
create index aux_state_employee_idx on agent_aux_state (employee_id, started_at desc);
create index aux_state_open_idx on agent_aux_state (employee_id) where ended_at is null;

create table cdr_records (
    id              uuid primary key default gen_random_uuid(),
    uniqueid        text not null unique,
    call_start      timestamptz not null,
    call_answer     timestamptz,
    call_end        timestamptz,
    direction       text not null default 'inbound' check (direction in ('inbound','outbound','internal')),
    queue           text,
    agent_ext       text,
    employee_id     uuid references employees(id) on delete set null,
    disposition     text,
    wait_seconds    int not null default 0,
    talk_seconds    int not null default 0,
    hold_seconds    int not null default 0,
    wrap_seconds    int not null default 0,
    abandoned       boolean not null default false,
    customer_number text,
    source_file     text,
    ingested_at     timestamptz not null default now()
);
create index cdr_start_idx on cdr_records (call_start desc);
create index cdr_employee_idx on cdr_records (employee_id, call_start desc);

create table qa_forms (
    id          uuid primary key default gen_random_uuid(),
    name        text not null,
    description text,
    active      boolean not null default true,
    created_at  timestamptz not null default now()
);

create table qa_criteria (
    id        uuid primary key default gen_random_uuid(),
    form_id   uuid not null references qa_forms(id) on delete cascade,
    name      text not null,
    weight    numeric(5,2) not null default 10,
    max_score int not null default 5,
    is_csat   boolean not null default false
);

create table qa_scores (
    id            uuid primary key default gen_random_uuid(),
    employee_id   uuid not null references employees(id) on delete cascade,
    evaluator_id  uuid references employees(id),
    cdr_id        uuid references cdr_records(id) on delete set null,
    form_id       uuid references qa_forms(id),
    scores        jsonb not null default '{}'::jsonb,
    total_score   numeric(6,2) not null default 0,
    weighted_pct  numeric(5,2) not null default 0,
    csat_score    numeric(3,2),
    comments      text,
    evaluated_at  timestamptz not null default now()
);
create index qa_scores_employee_idx on qa_scores (employee_id, evaluated_at desc);

create table kpi_snapshots (
    id                uuid primary key default gen_random_uuid(),
    captured_at       timestamptz not null default now(),
    clocked_in        int not null default 0,
    scheduled         int not null default 0,
    schedule_adherence numeric(5,2) not null default 0,
    aht_15m           numeric(7,2) not null default 0,
    aht_30m           numeric(7,2) not null default 0,
    queue_wait        int not null default 0,
    csat_avg          numeric(3,2) not null default 0,
    occupancy_pct     numeric(5,2) not null default 0,
    shrinkage_pct     numeric(5,2) not null default 0,
    calls_offered     int not null default 0,
    calls_answered    int not null default 0,
    abandoned_calls    int not null default 0
);
create index kpi_captured_idx on kpi_snapshots (captured_at desc);

-- Telephony extension used to map FreeSWITCH/Asterisk CDR agent_ext to an
-- employee (bulk import can also set it from the employee CSV template).
alter table employees add column extension text;
create unique index employees_extension_idx on employees (extension) where extension is not null;
