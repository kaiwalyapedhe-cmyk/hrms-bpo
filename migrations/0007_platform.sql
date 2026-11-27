-- 0007_platform.sql — login, RBAC, notifications and audit trail.

create table users (
    id             uuid primary key default gen_random_uuid(),
    employee_id    uuid references employees(id) on delete set null,
    email          text not null unique,
    name           text not null,
    password_hash  text not null,
    role           text not null check (role in (
                       'super_admin','hr_admin','payroll_admin','benefits_admin','recruiter',
                       'manager','team_lead','agent','qa_analyst','trainer')),
    active         boolean not null default true,
    last_login_at  timestamptz,
    created_at     timestamptz not null default now()
);

create table notifications (
    id           uuid primary key default gen_random_uuid(),
    employee_id  uuid references employees(id) on delete cascade,
    title        text not null,
    body         text not null,
    category     text not null default 'general',
    read         boolean not null default false,
    created_at   timestamptz not null default now()
);

create table audit_log (
    id          uuid primary key default gen_random_uuid(),
    actor_id    uuid,
    actor_email text,
    action      text not null,
    entity      text not null,
    entity_id   text,
    payload     jsonb not null default '{}'::jsonb,
    created_at  timestamptz not null default now()
);
create index audit_created_idx on audit_log (created_at desc);
