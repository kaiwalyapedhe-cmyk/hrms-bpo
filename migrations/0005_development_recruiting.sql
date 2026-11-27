-- 0005_development_recruiting.sql — Workforce Development (Training /
-- Enterprise Learning, ePerformance, Profile Management) and Recruiting
-- Solutions (Talent Acquisition Manager + Candidate Gateway).

create table courses (
    id           uuid primary key default gen_random_uuid(),
    code         text not null unique,
    title        text not null,
    category     text not null default 'compliance',
    mandatory    boolean not null default false,
    hours        numeric(5,2) not null default 1,
    description  text,
    active       boolean not null default true
);

create table course_sessions (
    id           uuid primary key default gen_random_uuid(),
    course_id    uuid not null references courses(id) on delete cascade,
    trainer_id   uuid references employees(id),
    start_at     timestamptz not null,
    end_at       timestamptz not null,
    mode         text not null default 'virtual' check (mode in ('virtual','classroom','self_paced')),
    location     text,
    capacity     int not null default 20,
    status       text not null default 'scheduled'
                 check (status in ('scheduled','completed','cancelled'))
);

create table training_enrollments (
    id            uuid primary key default gen_random_uuid(),
    employee_id   uuid not null references employees(id) on delete cascade,
    session_id    uuid not null references course_sessions(id) on delete cascade,
    status        text not null default 'enrolled'
                  check (status in ('enrolled','attended','completed','no_show','cancelled')),
    score         numeric(5,2),
    completed_at  timestamptz,
    unique (employee_id, session_id)
);

create table certifications (
    id           uuid primary key default gen_random_uuid(),
    employee_id  uuid not null references employees(id) on delete cascade,
    name         text not null,
    issuer       text,
    issued_on    date not null,
    expires_on   date,
    status       text not null default 'valid'
                 check (status in ('valid','expired','revoked')),
    created_at   timestamptz not null default now()
);

create table goals (
    id           uuid primary key default gen_random_uuid(),
    employee_id  uuid not null references employees(id) on delete cascade,
    cycle        text not null,
    title        text not null,
    description  text,
    weight       numeric(5,2) not null default 10,
    target       text,
    progress     int not null default 0 check (progress between 0 and 100),
    status       text not null default 'active'
                 check (status in ('draft','active','completed','cancelled')),
    created_at   timestamptz not null default now()
);

create table goal_checkpoints (
    id           uuid primary key default gen_random_uuid(),
    goal_id      uuid not null references goals(id) on delete cascade,
    note         text not null,
    rating       int check (rating between 1 and 5),
    created_by   uuid references employees(id),
    created_at   timestamptz not null default now()
);

create table performance_reviews (
    id            uuid primary key default gen_random_uuid(),
    employee_id   uuid not null references employees(id) on delete cascade,
    reviewer_id   uuid references employees(id),
    cycle         text not null,
    review_type   text not null default 'manager'
                  check (review_type in ('manager','self','peer','360')),
    rating        numeric(3,2),
    comments      text,
    status        text not null default 'draft'
                  check (status in ('draft','submitted','acknowledged')),
    created_at    timestamptz not null default now()
);

create table competencies (
    id           uuid primary key default gen_random_uuid(),
    code         text not null unique,
    name         text not null,
    category     text not null default 'functional',
    description  text
);

create table competency_ratings (
    id             uuid primary key default gen_random_uuid(),
    employee_id    uuid not null references employees(id) on delete cascade,
    competency_id  uuid not null references competencies(id),
    rating         int not null check (rating between 1 and 5),
    assessed_by    uuid references employees(id),
    assessed_on    date not null default current_date,
    unique (employee_id, competency_id)
);

create table requisitions (
    id             uuid primary key default gen_random_uuid(),
    req_no         text not null unique,
    title          text not null,
    department_id  uuid references departments(id),
    position_id    uuid references positions(id),
    location_id    uuid references locations(id),
    openings       int not null default 1,
    status         text not null default 'open'
                   check (status in ('draft','open','on_hold','filled','cancelled')),
    priority       text not null default 'medium' check (priority in ('low','medium','high')),
    recruiter_id   uuid references employees(id),
    opened_on      date not null default current_date,
    target_date    date,
    budget_annual  numeric(14,2) not null default 0,
    job_description text,
    created_at     timestamptz not null default now()
);

create table candidates (
    id             uuid primary key default gen_random_uuid(),
    first_name     text not null,
    last_name      text not null,
    email          text not null,
    phone          text,
    source         text not null default 'portal',
    requisition_id uuid references requisitions(id) on delete set null,
    stage          text not null default 'applied'
                   check (stage in ('applied','screening','interview','offer','hired','rejected','withdrawn')),
    rating         numeric(3,2),
    resume_key     text,
    created_at     timestamptz not null default now(),
    unique (email, requisition_id)
);

create table interviews (
    id              uuid primary key default gen_random_uuid(),
    candidate_id    uuid not null references candidates(id) on delete cascade,
    requisition_id  uuid references requisitions(id) on delete set null,
    round           int not null default 1,
    scheduled_at    timestamptz not null,
    mode            text not null default 'virtual' check (mode in ('virtual','in_person','phone')),
    interviewer_id  uuid references employees(id),
    feedback        text,
    score           numeric(3,2),
    status          text not null default 'scheduled'
                    check (status in ('scheduled','completed','no_show','cancelled'))
);

create table offers (
    id             uuid primary key default gen_random_uuid(),
    candidate_id   uuid not null references candidates(id) on delete cascade,
    requisition_id uuid references requisitions(id) on delete set null,
    position_id    uuid references positions(id),
    annual_ctc     numeric(14,2) not null,
    offer_date     date not null default current_date,
    expires_on     date,
    status         text not null default 'draft'
                   check (status in ('draft','released','accepted','declined','expired')),
    accepted_on    date,
    created_at     timestamptz not null default now()
);
