//! Idempotent demo seed: one SuperAdmin, 20 employees across four states,
//! attendance data for today, leave, benefits, training and recruiting
//! and a fully computed payroll run for last month.
//!
//! Usage: `DATABASE_URL=... cargo run -p hrms-seed` (or the `seed` compose
//! profile). Re-running is safe: it exits early if the admin account exists.

use std::collections::HashMap;

use anyhow::{Context, Result};
use argon2::password_hash::{rand_core::OsRng, SaltString};
use argon2::{Argon2, PasswordHasher};
use chrono::{Datelike, Duration, NaiveDate, TimeZone, Utc};
use hrms_core::{
    EmploymentType, Gender, IndiaPayrollConfig, PayPeriod, PayrollEngine, PayrollInput, TaxRegime,
};
use rust_decimal::Decimal;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

#[derive(Clone, Copy)]
struct EmployeeSeed {
    emp_no: &'static str,
    first: &'static str,
    last: &'static str,
    gender: &'static str,
    title: &'static str,
    department: &'static str,
    location: &'static str,
    work_state: &'static str,
    annual_ctc: i64,
    joined: &'static str,
    employment_type: &'static str,
    manager: Option<&'static str>,
}

const EMPLOYEES: [EmployeeSeed; 20] = [
    EmployeeSeed { emp_no: "EMP-1001", first: "Ananya", last: "Rao", gender: "female", title: "OPS_MANAGER", department: "CUSTOPS", location: "BLR", work_state: "KA", annual_ctc: 1_800_000, joined: "2019-04-01", employment_type: "full_time", manager: None },
    EmployeeSeed { emp_no: "EMP-1002", first: "Rahul", last: "Menon", gender: "male", title: "TEAM_LEAD", department: "CUSTOPS", location: "BLR", work_state: "KA", annual_ctc: 900_000, joined: "2020-06-15", employment_type: "full_time", manager: Some("EMP-1001") },
    EmployeeSeed { emp_no: "EMP-1003", first: "Priya", last: "Nair", gender: "female", title: "TEAM_LEAD", department: "CUSTOPS", location: "BOM", work_state: "MH", annual_ctc: 850_000, joined: "2020-09-01", employment_type: "full_time", manager: Some("EMP-1001") },
    EmployeeSeed { emp_no: "EMP-1004", first: "Arjun", last: "Sharma", gender: "male", title: "AGENT", department: "CUSTOPS", location: "BLR", work_state: "KA", annual_ctc: 420_000, joined: "2021-01-11", employment_type: "full_time", manager: Some("EMP-1002") },
    EmployeeSeed { emp_no: "EMP-1005", first: "Sneha", last: "Iyer", gender: "female", title: "AGENT", department: "CUSTOPS", location: "BLR", work_state: "KA", annual_ctc: 400_000, joined: "2021-02-08", employment_type: "full_time", manager: Some("EMP-1002") },
    EmployeeSeed { emp_no: "EMP-1006", first: "Vikram", last: "Patil", gender: "male", title: "AGENT", department: "CUSTOPS", location: "BOM", work_state: "MH", annual_ctc: 450_000, joined: "2021-03-15", employment_type: "full_time", manager: Some("EMP-1003") },
    EmployeeSeed { emp_no: "EMP-1007", first: "Meera", last: "Kulkarni", gender: "female", title: "AGENT", department: "CUSTOPS", location: "BOM", work_state: "MH", annual_ctc: 380_000, joined: "2021-07-01", employment_type: "full_time", manager: Some("EMP-1003") },
    EmployeeSeed { emp_no: "EMP-1008", first: "Rohit", last: "Banerjee", gender: "male", title: "AGENT", department: "CUSTOPS", location: "CCU", work_state: "WB", annual_ctc: 360_000, joined: "2021-08-16", employment_type: "full_time", manager: Some("EMP-1003") },
    EmployeeSeed { emp_no: "EMP-1009", first: "Tanvi", last: "Chatterjee", gender: "female", title: "AGENT", department: "CUSTOPS", location: "CCU", work_state: "WB", annual_ctc: 355_000, joined: "2021-09-06", employment_type: "full_time", manager: Some("EMP-1003") },
    EmployeeSeed { emp_no: "EMP-1010", first: "Karthik", last: "Subramanian", gender: "male", title: "AGENT", department: "CUSTOPS", location: "MAA", work_state: "TN", annual_ctc: 365_000, joined: "2021-10-04", employment_type: "full_time", manager: Some("EMP-1002") },
    EmployeeSeed { emp_no: "EMP-1011", first: "Divya", last: "Ramesh", gender: "female", title: "AGENT", department: "CUSTOPS", location: "MAA", work_state: "TN", annual_ctc: 350_000, joined: "2022-01-10", employment_type: "fixed_term", manager: Some("EMP-1002") },
    EmployeeSeed { emp_no: "EMP-1012", first: "Aditya", last: "Joshi", gender: "male", title: "AGENT", department: "CUSTOPS", location: "BLR", work_state: "KA", annual_ctc: 480_000, joined: "2022-02-14", employment_type: "full_time", manager: Some("EMP-1002") },
    EmployeeSeed { emp_no: "EMP-1013", first: "Nisha", last: "Verma", gender: "female", title: "AGENT", department: "CUSTOPS", location: "BLR", work_state: "KA", annual_ctc: 410_000, joined: "2022-05-09", employment_type: "full_time", manager: Some("EMP-1002") },
    EmployeeSeed { emp_no: "EMP-1014", first: "Sameer", last: "Khan", gender: "male", title: "AGENT", department: "CUSTOPS", location: "BOM", work_state: "MH", annual_ctc: 390_000, joined: "2022-06-20", employment_type: "full_time", manager: Some("EMP-1003") },
    EmployeeSeed { emp_no: "EMP-1015", first: "Pooja", last: "Desai", gender: "female", title: "AGENT", department: "CUSTOPS", location: "BOM", work_state: "MH", annual_ctc: 375_000, joined: "2022-07-18", employment_type: "full_time", manager: Some("EMP-1003") },
    EmployeeSeed { emp_no: "EMP-1016", first: "Manish", last: "Gupta", gender: "male", title: "AGENT", department: "CUSTOPS", location: "CCU", work_state: "WB", annual_ctc: 345_000, joined: "2023-01-09", employment_type: "full_time", manager: Some("EMP-1003") },
    EmployeeSeed { emp_no: "EMP-1017", first: "Aisha", last: "Siddiqui", gender: "female", title: "QA_ANALYST", department: "QA", location: "BLR", work_state: "KA", annual_ctc: 620_000, joined: "2021-11-02", employment_type: "full_time", manager: Some("EMP-1001") },
    EmployeeSeed { emp_no: "EMP-1018", first: "Raghav", last: "Pillai", gender: "male", title: "TRAINER", department: "LND", location: "BLR", work_state: "KA", annual_ctc: 640_000, joined: "2020-12-07", employment_type: "full_time", manager: Some("EMP-1001") },
    EmployeeSeed { emp_no: "EMP-1019", first: "Lakshmi", last: "Hegde", gender: "female", title: "PAYROLL_OFFICER", department: "HR", location: "BLR", work_state: "KA", annual_ctc: 720_000, joined: "2019-11-25", employment_type: "full_time", manager: Some("EMP-1001") },
    EmployeeSeed { emp_no: "EMP-1020", first: "Nikhil", last: "Bose", gender: "male", title: "RECRUITER", department: "HR", location: "BLR", work_state: "KA", annual_ctc: 560_000, joined: "2022-03-21", employment_type: "full_time", manager: Some("EMP-1001") },
];

const DEPARTMENTS: [(&str, &str, &str); 5] = [
    ("CUSTOPS", "Customer Operations", "CC-1001"),
    ("QA", "Quality Assurance", "CC-2001"),
    ("LND", "Learning & Development", "CC-3001"),
    ("HR", "Human Resources", "CC-4001"),
    ("WFM", "Workforce Management", "CC-5001"),
];

const LOCATIONS: [(&str, &str, &str, &str); 4] = [
    ("BLR", "Bengaluru — Embassy Tech Village", "Bengaluru", "KA"),
    ("BOM", "Mumbai — Powai Campus", "Mumbai", "MH"),
    ("CCU", "Kolkata — Sector V", "Kolkata", "WB"),
    ("MAA", "Chennai — Guindy", "Chennai", "TN"),
];

const POSITION_TITLES: [(&str, &str, i64); 8] = [
    ("OPS_MANAGER", "Operations Manager", 1_800_000),
    ("TEAM_LEAD", "Team Leader", 900_000),
    ("AGENT", "Customer Support Agent", 420_000),
    ("QA_ANALYST", "Quality Analyst", 620_000),
    ("TRAINER", "Process Trainer", 640_000),
    ("PAYROLL_OFFICER", "Payroll Officer", 720_000),
    ("RECRUITER", "Talent Acquisition Specialist", 560_000),
    ("WFM_ANALYST", "WFM Analyst", 660_000),
];

fn dec_int(value: i64) -> Decimal {
    Decimal::from(value)
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let database_url =
        std::env::var("DATABASE_URL").context("DATABASE_URL must be set to seed the database")?;
    let password = std::env::var("SEED_PASSWORD").unwrap_or_else(|_| "ChangeMe#2026".to_string());

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

    sqlx::migrate!("../../migrations")
        .run(&pool)
        .await
        .context("running migrations")?;

    let already: i64 =
        sqlx::query_scalar("select count(*) from users where email = 'admin@hrms.local'")
            .fetch_one(&pool)
            .await?;
    if already > 0 {
        println!("Seed data already present (admin@hrms.local exists) — nothing to do.");
        return Ok(());
    }

    let config = IndiaPayrollConfig::load();
    let engine = PayrollEngine::new(config.clone());
    let today = Utc::now().date_naive();
    let year = today.year();

    // ------------------------------------------------------------------
    // Organisation structure
    // ------------------------------------------------------------------
    let mut departments = HashMap::new();
    for (code, name, cost_center) in DEPARTMENTS {
        let id: Uuid = sqlx::query_scalar(
            "insert into departments (code, name, cost_center) values ($1,$2,$3) returning id",
        )
        .bind(code)
        .bind(name)
        .bind(cost_center)
        .fetch_one(&pool)
        .await?;
        departments.insert(code, id);
    }

    let mut locations = HashMap::new();
    for (code, name, city, state) in LOCATIONS {
        let id: Uuid = sqlx::query_scalar(
            "insert into locations (code, name, city, state_code) values ($1,$2,$3,$4) returning id",
        )
        .bind(code)
        .bind(name)
        .bind(city)
        .bind(state)
        .fetch_one(&pool)
        .await?;
        locations.insert(code, id);
    }

    let mut positions: HashMap<(&'static str, &'static str), Uuid> = HashMap::new();
    for (title, display, budget) in POSITION_TITLES {
        for (loc_code, _, _, _) in LOCATIONS {
            let code = format!("POS-{title}-{loc_code}");
            let department = if title == "QA_ANALYST" {
                "QA"
            } else if title == "TRAINER" {
                "LND"
            } else if matches!(title, "PAYROLL_OFFICER" | "RECRUITER") {
                "HR"
            } else if title == "WFM_ANALYST" {
                "WFM"
            } else {
                "CUSTOPS"
            };
            let id: Uuid = sqlx::query_scalar(
                r#"insert into positions (code, title, department_id, location_id, budgeted_annual_salary, status)
                   values ($1,$2,$3,$4,$5,'approved') returning id"#,
            )
            .bind(&code)
            .bind(display)
            .bind(departments[department])
            .bind(locations[loc_code])
            .bind(dec_int(budget))
            .fetch_one(&pool)
            .await?;
            positions.insert((title, loc_code), id);
        }
    }

    // ------------------------------------------------------------------
    // People
    // ------------------------------------------------------------------
    let mut employees: HashMap<&'static str, Uuid> = HashMap::new();
    for seed in &EMPLOYEES {
        let id = Uuid::new_v4();
        employees.insert(seed.emp_no, id);
    }
    for seed in &EMPLOYEES {
        let id = employees[seed.emp_no];
        let manager_id = seed.manager.map(|no| employees[no]);
        let joined = NaiveDate::parse_from_str(seed.joined, "%Y-%m-%d")?;
        sqlx::query(
            r#"insert into employees
               (id, emp_no, first_name, last_name, email, phone, date_of_birth, gender, date_of_joining,
                status, employment_type, position_id, manager_id, department_id, location_id, work_state,
                pan, uan, esic_ip, pf_exempt, esi_exempt, tax_regime, disability, bank_account, ifsc)
               values ($1,$2,$3,$4,$5,$6,$7,$8,$9,'active',$10,$11,$12,$13,$14,$15,$16,$17,$18,false,false,'new',false,$19,$20)"#,
        )
        .bind(id)
        .bind(seed.emp_no)
        .bind(seed.first)
        .bind(seed.last)
        .bind(format!(
            "{}.{}@hrms.local",
            seed.first.to_lowercase(),
            seed.last.to_lowercase()
        ))
        .bind(format!("+9198{:08}", 10_000_000 + seed.emp_no[4..].parse::<i32>()?))
        .bind(joined - Duration::days(9_000))
        .bind(seed.gender)
        .bind(joined)
        .bind(seed.employment_type)
        .bind(positions[&(seed.title, seed.location)])
        .bind(manager_id)
        .bind(departments[seed.department])
        .bind(locations[seed.location])
        .bind(seed.work_state)
        .bind(format!("ABCDE{:04}F", seed.emp_no[4..].parse::<i32>()?))
        .bind(format!("10{:010}", seed.emp_no[4..].parse::<i64>()?))
        .bind(format!("53{:015}", seed.emp_no[4..].parse::<i64>()?))
        .bind(format!("91{:011}", seed.emp_no[4..].parse::<i64>()?))
        .bind("HDFC0001234")
        .execute(&pool)
        .await?;
    }

    // ------------------------------------------------------------------
    // Compensation (50% basic per Code on Wages)
    // ------------------------------------------------------------------
    let structure_id: Uuid = sqlx::query_scalar(
        "insert into salary_structures (code, name, effective_from, basic_pct, hra_pct, conveyance_pct, special_pct, notes)
         values ('STD-50','Standard 50% basic','2024-04-01',50,20,5,25,'Code on Wages: basic >= 50% of remuneration') returning id",
    )
    .fetch_one(&pool)
    .await?;

    for seed in &EMPLOYEES {
        let annual = Decimal::from(seed.annual_ctc);
        let monthly = (annual / Decimal::from(12)).round_dp(2);
        let basic = (monthly * Decimal::new(50, 2)).round_dp(2);
        let hra = (monthly * Decimal::new(20, 2)).round_dp(2);
        let conveyance = (monthly * Decimal::new(5, 2)).round_dp(2);
        let special = monthly - basic - hra - conveyance;
        sqlx::query(
            r#"insert into employee_compensation
               (employee_id, structure_id, annual_remuneration, basic, hra, conveyance, special_allowance, other_allowances, effective_from)
               values ($1,$2,$3,$4,$5,$6,$7,0,'2024-04-01')"#,
        )
        .bind(employees[seed.emp_no])
        .bind(structure_id)
        .bind(annual)
        .bind(basic)
        .bind(hra)
        .bind(conveyance)
        .bind(special)
        .execute(&pool)
        .await?;
    }

    // ------------------------------------------------------------------
    // Leave types, balances and requests
    // ------------------------------------------------------------------
    let mut leave_types = HashMap::new();
    for (code, name, quota, monthly, paid) in [
        ("EL", "Earned Leave", 21.0, 1.75, true),
        ("SL", "Sick Leave", 12.0, 1.0, true),
        ("CL", "Casual Leave", 7.0, 0.58, true),
        ("MAT", "Maternity Leave", 182.0, 0.0, true),
    ] {
        let id: Uuid = sqlx::query_scalar(
            "insert into leave_types (code, name, annual_quota, monthly_accrual, carry_forward, encashable, paid, max_balance)
             values ($1,$2,$3,$4,true,$5,$6,$7) returning id",
        )
        .bind(code)
        .bind(name)
        .bind(Decimal::try_from(quota).unwrap_or(Decimal::ZERO))
        .bind(Decimal::try_from(monthly).unwrap_or(Decimal::ZERO))
        .bind(code == "EL")
        .bind(paid)
        .bind(Decimal::try_from(if code == "EL" { 45.0 } else { quota }).unwrap_or(Decimal::ZERO))
        .fetch_one(&pool)
        .await?;
        leave_types.insert(code, id);
    }

    for (index, seed) in EMPLOYEES.iter().enumerate() {
        let used_el = Decimal::from((index % 6) as i64);
        sqlx::query(
            r#"insert into leave_balances (employee_id, leave_type_id, year, accrued, used, available)
               values ($1,$2,$3,$4,$5,$4) , ($1,$6,$3,$7,$8,$7), ($1,$9,$3,$10,$11,$10)"#,
        )
        .bind(employees[seed.emp_no])
        .bind(leave_types["EL"])
        .bind(year)
        .bind(Decimal::from(21))
        .bind(used_el)
        .bind(leave_types["SL"])
        .bind(Decimal::from(12))
        .bind(Decimal::from((index % 3) as i64))
        .bind(leave_types["CL"])
        .bind(Decimal::from(7))
        .bind(Decimal::from((index % 2) as i64))
        .execute(&pool)
        .await?;
        let _ = sqlx::query(
            "update leave_balances set available = accrued - used where employee_id = $1 and year = $2",
        )
        .bind(employees[seed.emp_no])
        .bind(year)
        .execute(&pool)
        .await;
    }

    for (offset, code, status) in [
        (2i64, "EL", "pending"),
        (5, "SL", "approved"),
        (11, "EL", "approved"),
        (16, "CL", "pending"),
        (23, "SL", "approved"),
        (30, "EL", "pending"),
    ] {
        let seed = &EMPLOYEES[3 + (offset as usize % 17)];
        let from = today + Duration::days(offset);
        let to = from + Duration::days(1);
        sqlx::query(
            r#"insert into leave_requests
               (employee_id, leave_type_id, from_date, to_date, days, reason, status, approver_id, decided_at)
               values ($1,$2,$3,$4,2,$5,$6,$7,$8)"#,
        )
        .bind(employees[seed.emp_no])
        .bind(leave_types[code])
        .bind(from)
        .bind(to)
        .bind(match code {
            "SL" => "Fever — doctor advised rest",
            "CL" => "Family function",
            _ => "Personal travel",
        })
        .bind(status)
        .bind(if status == "approved" {
            Some(employees["EMP-1002"])
        } else {
            None
        })
        .bind(if status == "approved" {
            Some(Utc::now() - Duration::hours(6))
        } else {
            None
        })
        .execute(&pool)
        .await?;
    }

    for (date, name) in [
        ("2026-01-26", "Republic Day"),
        ("2026-08-15", "Independence Day"),
        ("2026-10-02", "Gandhi Jayanti"),
    ] {
        sqlx::query(
            "insert into holidays (holiday_date, name, location_id) values ($1,$2,null)",
        )
        .bind(NaiveDate::parse_from_str(date, "%Y-%m-%d")?)
        .bind(name)
        .execute(&pool)
        .await?;
    }

    // ------------------------------------------------------------------
    // Shifts, assignments, punches, timesheets
    // ------------------------------------------------------------------
    let shift_morning: Uuid = sqlx::query_scalar(
        "insert into shifts (code, name, start_time, end_time, break_minutes, grace_minutes, is_night)
         values ('MORN','Morning 9-6','09:00','18:00',45,5,false) returning id",
    )
    .fetch_one(&pool)
    .await?;
    let shift_evening: Uuid = sqlx::query_scalar(
        "insert into shifts (code, name, start_time, end_time, break_minutes, grace_minutes, is_night)
         values ('EVE','Evening 1-10','13:00','22:00',45,5,false) returning id",
    )
    .fetch_one(&pool)
    .await?;
    let shift_night: Uuid = sqlx::query_scalar(
        "insert into shifts (code, name, start_time, end_time, break_minutes, grace_minutes, is_night)
         values ('NIGHT','Night 10-7','22:00','07:00',60,10,true) returning id",
    )
    .fetch_one(&pool)
    .await?;
    let shifts = [shift_morning, shift_evening, shift_night];

    for (index, seed) in EMPLOYEES.iter().enumerate() {
        let employee_id = employees[seed.emp_no];
        let shift = shifts[index % 3];
        for day_offset in 0..7 {
            let date = today - Duration::days(day_offset);
            if date.weekday().number_from_monday() == 7 {
                continue;
            }
            sqlx::query(
                "insert into shift_assignments (employee_id, shift_id, work_date) values ($1,$2,$3)
                 on conflict do nothing",
            )
            .bind(employee_id)
            .bind(shift)
            .bind(date)
            .execute(&pool)
            .await?;
        }
    }

    for (index, seed) in EMPLOYEES.iter().enumerate() {
        let employee_id = employees[seed.emp_no];
        let day_start = Utc::now()
            .date_naive()
            .and_hms_opt(9, 0, 0)
            .expect("valid time");
        let started = Utc.from_utc_datetime(&day_start) + Duration::minutes((index % 9) as i64);
        for (punch_type, minutes) in [
            ("in", 0),
            ("break_start", 240),
            ("break_end", 285),
        ] {
            sqlx::query(
                "insert into punches (employee_id, punch_time, punch_type, source) values ($1,$2,$3,'biometric')",
            )
            .bind(employee_id)
            .bind(started + Duration::minutes(minutes))
            .bind(punch_type)
            .execute(&pool)
            .await?;
        }

        let yesterday = today - Duration::days(1);
        let in_time = Utc.from_utc_datetime(
            &yesterday.and_hms_opt(9, 0, 0).expect("valid time"),
        ) + Duration::minutes((index % 7) as i64);
        let out_time = in_time + Duration::hours(9) + Duration::minutes(10);
        sqlx::query(
            r#"insert into timesheets
               (employee_id, work_date, shift_id, in_time, out_time, break_minutes, regular_hours, ot_hours,
                status, approved_by, approved_at, notes)
               values ($1,$2,$3,$4,$5,45,'8.00',$6,'approved',$7,now(),$8)"#,
        )
        .bind(employee_id)
        .bind(yesterday)
        .bind(shifts[index % 3])
        .bind(in_time)
        .bind(out_time)
        .bind(if index % 5 == 0 { Decimal::from(2) } else { Decimal::ZERO })
        .bind(employees["EMP-1001"])
        .bind(if index % 5 == 0 {
            Some("Extra queue coverage")
        } else {
            None
        })
        .execute(&pool)
        .await?;
    }

    for (offset, seed) in EMPLOYEES.iter().skip(3).take(4).enumerate() {
        sqlx::query(
            r#"insert into ot_requests (employee_id, work_date, hours, reason, status, approver_id, decided_at)
               values ($1,$2,$3,'Month-end campaign', $4, $5, $6)"#,
        )
        .bind(employees[seed.emp_no])
        .bind(today - Duration::days(offset as i64))
        .bind(Decimal::from(2))
        .bind(if offset % 2 == 0 { "approved" } else { "pending" })
        .bind(if offset % 2 == 0 {
            Some(employees["EMP-1002"])
        } else {
            None
        })
        .bind(if offset % 2 == 0 { Some(Utc::now()) } else { None })
        .execute(&pool)
        .await?;
    }

    // ------------------------------------------------------------------
    // Payroll: fully computed run for last month
    // ------------------------------------------------------------------
    let (run_year, run_month) = if today.month() == 1 {
        (today.year() - 1, 12u32)
    } else {
        (today.year(), today.month() - 1)
    };
    let period = PayPeriod::new(run_year, run_month);
    let payroll_run_id = Uuid::new_v4();
    let mut total_gross = Decimal::ZERO;
    let mut total_net = Decimal::ZERO;
    let mut total_employer = Decimal::ZERO;
    let mut total_tds = Decimal::ZERO;

    // Create the run row first so payslips can reference it (FK).
    sqlx::query(
        r#"insert into payroll_runs
           (id, period_year, period_month, status, run_at, employee_count, total_gross, total_net,
            total_employer_cost, total_tds, notes)
           values ($1,$2,$3,'processing',now(),0,0,0,0,0,'Seeded demo payroll — computing')"#,
    )
    .bind(payroll_run_id)
    .bind(run_year)
    .bind(run_month as i32)
    .execute(&pool)
    .await?;

    for seed in &EMPLOYEES {
        let annual = Decimal::from(seed.annual_ctc);
        let monthly = (annual / Decimal::from(12)).round_dp(2);
        let basic = (monthly * Decimal::new(50, 2)).round_dp(2);
        let hra = (monthly * Decimal::new(20, 2)).round_dp(2);
        let conveyance = (monthly * Decimal::new(5, 2)).round_dp(2);
        let special = monthly - basic - hra - conveyance;

        let mut input = PayrollInput::new(employees[seed.emp_no], period);
        input.basic = basic;
        input.hra = hra;
        input.conveyance = conveyance;
        input.special_allowance = special;
        input.work_state = seed.work_state.to_string();
        input.gender = match seed.gender {
            "female" => Gender::Female,
            _ => Gender::Male,
        };
        input.employment_type =
            EmploymentType::from_str_opt(seed.employment_type).unwrap_or(EmploymentType::FullTime);
        input.tax_regime = TaxRegime::New;
        input.pf_applicable = true;
        input.esi_applicable = true;
        input.ctc_annual = Some(annual);
        input.annual_gross_projection = Some(annual);
        input.service_years = ((today - NaiveDate::parse_from_str(seed.joined, "%Y-%m-%d")?).num_days()
            / 365) as u32;
        if seed.title == "AGENT" {
            input.incentive = Decimal::from(1_500);
            input.overtime_hours = Decimal::from(2);
        }

        let computation = engine
            .compute(&input)
            .with_context(|| format!("computing payroll for {}", seed.emp_no))?;
        let payslip_id = Uuid::new_v4();
        sqlx::query(
            r#"insert into payslips
               (id, run_id, employee_id, period_year, period_month, days_in_month, paid_days, lop_days,
                gross, total_deductions, net_pay, employer_cost, pf_wages, pf_employee, pf_employer_epf,
                pf_employer_eps, edli, pf_admin, esi_wages, esi_employee, esi_employer, pt_state, pt_amount,
                tds, gratuity_accrual, taxable_income_annual, tax_regime, is_reversal, status, breakdown)
               values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$24,$25,$26,$27,false,'paid',$28)"#,
        )
        .bind(payslip_id)
        .bind(payroll_run_id)
        .bind(computation.employee_id)
        .bind(period.year)
        .bind(period.month as i32)
        .bind(computation.days_in_month as i32)
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
        .bind(if computation.professional_tax.applicable {
            Some(seed.work_state)
        } else {
            None::<&str>
        })
        .bind(computation.professional_tax.amount)
        .bind(computation.tds_monthly)
        .bind(computation.gratuity_accrual)
        .bind(computation.tds.taxable_income)
        .bind(computation.tds.regime.clone())
        .bind(serde_json::to_value(&computation)?)
        .execute(&pool)
        .await?;

        for message in &computation.messages {
            sqlx::query(
                "insert into payroll_messages (payslip_id, code, severity, message) values ($1,$2,$3,$4)",
            )
            .bind(payslip_id)
            .bind(&message.code)
            .bind(&message.severity)
            .bind(&message.text)
            .execute(&pool)
            .await?;
        }

        total_gross += computation.gross_earnings;
        total_net += computation.net_pay;
        total_employer += computation.employer_cost;
        total_tds += computation.tds_monthly;
    }

    sqlx::query(
        r#"update payroll_runs
           set status='posted', run_at=now(), employee_count=$2, total_gross=$3, total_net=$4,
               total_employer_cost=$5, total_tds=$6, notes='Seeded demo payroll — computed by the statutory engine'
           where id=$1"#,
    )
    .bind(payroll_run_id)
    .bind(EMPLOYEES.len() as i32)
    .bind(total_gross)
    .bind(total_net)
    .bind(total_employer)
    .bind(total_tds)
    .execute(&pool)
    .await?;

    // ------------------------------------------------------------------
    // Benefits, development and recruiting samples
    // ------------------------------------------------------------------
    let mut plans = Vec::new();
    for (code, name, provider, category, employee_cost, employer_cost, coverage) in [
        ("MEDI-BASE", "Group Mediclaim — Base", "Star Health", "health", 450.0, 1_200.0, 500_000.0),
        ("GPA-LIFE", "Group Personal Accident", "ICICI Lombard", "accident", 120.0, 300.0, 1_000_000.0),
        ("GYM-2026", "Wellness Allowance", "Cult.fit", "wellness", 0.0, 1_000.0, 0.0),
    ] {
        let id: Uuid = sqlx::query_scalar(
            r#"insert into benefit_plans
               (code, name, provider, category, employee_cost, employer_cost, coverage_amount, description)
               values ($1,$2,$3,$4,$5,$6,$7,'Seeded plan') returning id"#,
        )
        .bind(code)
        .bind(name)
        .bind(provider)
        .bind(category)
        .bind(Decimal::try_from(employee_cost).unwrap_or(Decimal::ZERO))
        .bind(Decimal::try_from(employer_cost).unwrap_or(Decimal::ZERO))
        .bind(Decimal::try_from(coverage).unwrap_or(Decimal::ZERO))
        .fetch_one(&pool)
        .await?;
        plans.push(id);
    }
    for (index, seed) in EMPLOYEES.iter().enumerate() {
        sqlx::query(
            r#"insert into benefit_enrollments
               (employee_id, plan_id, effective_from, employee_cost, employer_cost, dependents_covered, status)
               values ($1,$2,'2026-04-01',450,1200,$3,'active')"#,
        )
        .bind(employees[seed.emp_no])
        .bind(plans[0])
        .bind((index % 3) as i32)
        .execute(&pool)
        .await?;
        if index % 4 == 0 {
            sqlx::query(
                "insert into dependents (employee_id, name, relation, date_of_birth) values ($1,$2,'spouse',$3)",
            )
            .bind(employees[seed.emp_no])
            .bind(format!("{} {}", seed.first, "Spouse"))
            .bind(NaiveDate::from_ymd_opt(1992, 6, 12))
            .execute(&pool)
            .await?;
        }
    }

    let mut sessions = Vec::new();
    for (code, title, mandatory, hours) in [
        ("POSH-101", "Prevention of Sexual Harassment", true, 2.0),
        ("DPDP-101", "Data Privacy & DPDP Act", true, 1.5),
        ("CALL-QUAL", "Consultative Call Handling", false, 4.0),
    ] {
        let course_id: Uuid = sqlx::query_scalar(
            "insert into courses (code, title, category, mandatory, hours, description) values ($1,$2,'compliance',$3,$4,'Seeded course') returning id",
        )
        .bind(code)
        .bind(title)
        .bind(mandatory)
        .bind(Decimal::try_from(hours).unwrap_or(Decimal::ZERO))
        .fetch_one(&pool)
        .await?;
        let session_id: Uuid = sqlx::query_scalar(
            "insert into course_sessions (course_id, trainer_id, start_at, end_at, mode, location, capacity, status)
             values ($1,$2,now() - interval '14 days', now() - interval '14 days' + interval '2 hours','virtual','Teams',30,'completed') returning id",
        )
        .bind(course_id)
        .bind(employees["EMP-1018"])
        .fetch_one(&pool)
        .await?;
        sessions.push((course_id, session_id));
    }
    for (index, seed) in EMPLOYEES.iter().enumerate() {
        let (course_id, session_id) = sessions[index % sessions.len()];
        sqlx::query(
            r#"insert into training_enrollments (employee_id, session_id, status, score, completed_at)
               values ($1,$2,'completed',$3,now() - interval '13 days')"#,
        )
        .bind(employees[seed.emp_no])
        .bind(session_id)
        .bind(Decimal::from(70 + (index % 25) as i64))
        .execute(&pool)
        .await?;
        if index % 5 == 0 {
            sqlx::query(
                r#"insert into certifications (employee_id, name, issuer, issued_on, expires_on, status)
                   values ($1,'ISO 9001 Internal Auditor','Bureau Veritas','2025-05-10','2027-05-09','valid')"#,
            )
            .bind(employees[seed.emp_no])
            .execute(&pool)
            .await?;
        }
        let _ = course_id;
    }

    for (index, seed) in EMPLOYEES.iter().skip(3).take(10).enumerate() {
        let goal_id: Uuid = sqlx::query_scalar(
            r#"insert into goals (employee_id, cycle, title, description, weight, target, progress, status)
               values ($1,'FY2026-27','Improve CSAT','Raise rolling CSAT above 4.6',40,'4.6 / 5', $2, 'active') returning id"#,
        )
        .bind(employees[seed.emp_no])
        .bind((30 + index * 5) as i32)
        .fetch_one(&pool)
        .await?;
        sqlx::query(
            "insert into goal_checkpoints (goal_id, note, rating, created_by) values ($1,'Mid-cycle checkpoint: trending up',4,$2)",
        )
        .bind(goal_id)
        .bind(employees["EMP-1002"])
        .execute(&pool)
        .await?;
        sqlx::query(
            "insert into performance_reviews (employee_id, reviewer_id, cycle, review_type, rating, comments, status)
             values ($1,$2,'FY2025-26','manager',4.0,'Consistently meets quality and adherence targets','acknowledged')",
        )
        .bind(employees[seed.emp_no])
        .bind(employees["EMP-1002"])
        .execute(&pool)
        .await?;
    }

    let mut competencies = Vec::new();
    for (code, name, category) in [
        ("CMP-EMPATHY", "Customer Empathy", "behavioural"),
        ("CMP-PROD", "Product Knowledge", "functional"),
        ("CMP-COMMS", "Communication", "behavioural"),
        ("CMP-RESOLVE", "Problem Resolution", "functional"),
        ("CMP-COMPLIANCE", "Regulatory Compliance", "functional"),
        ("CMP-TEAMWORK", "Teamwork", "behavioural"),
    ] {
        let id: Uuid = sqlx::query_scalar(
            "insert into competencies (code, name, category) values ($1,$2,$3) returning id",
        )
        .bind(code)
        .bind(name)
        .bind(category)
        .fetch_one(&pool)
        .await?;
        competencies.push(id);
    }
    for (index, seed) in EMPLOYEES.iter().skip(3).take(8).enumerate() {
        for (position, competency) in competencies.iter().enumerate() {
            sqlx::query(
                "insert into competency_ratings (employee_id, competency_id, rating, assessed_by)
                 values ($1,$2,$3,$4) on conflict do nothing",
            )
            .bind(employees[seed.emp_no])
            .bind(competency)
            .bind((3 + ((index + position) % 3)) as i32)
            .bind(employees["EMP-1002"])
            .execute(&pool)
            .await?;
        }
    }

    let mut requisitions = Vec::new();
    for (index, (title, openings, budget, priority)) in [
        ("Voice Support Agent — Bengaluru", 6, 420_000.0, "high"),
        ("Quality Analyst — Mumbai", 1, 620_000.0, "medium"),
        ("Team Leader — Kolkata", 2, 850_000.0, "high"),
    ]
    .into_iter()
    .enumerate()
    {
        let id: Uuid = sqlx::query_scalar(
            r#"insert into requisitions
               (req_no, title, department_id, location_id, openings, status, priority, recruiter_id,
                opened_on, target_date, budget_annual, job_description)
               values ($1,$2,$3,$4,$5,'open',$6,$7,current_date - 12, current_date + 30, $8, 'Seeded requisition') returning id"#,
        )
        .bind(format!("REQ-{}-{:03}", today.format("%Y%m"), index + 1))
        .bind(title)
        .bind(departments["CUSTOPS"])
        .bind(locations["BLR"])
        .bind(openings)
        .bind(priority)
        .bind(employees["EMP-1020"])
        .bind(Decimal::try_from(budget).unwrap_or(Decimal::ZERO))
        .fetch_one(&pool)
        .await?;
        requisitions.push(id);
    }

    for index in 0..6 {
        let candidate_id: Uuid = sqlx::query_scalar(
            r#"insert into candidates (first_name, last_name, email, phone, source, requisition_id, stage, rating)
               values ($1,$2,$3,$4,'careers_portal',$5,$6,$7) returning id"#,
        )
        .bind(["Isha", "Dev", "Farah", "Gaurav", "Heena", "Imran"][index])
        .bind(["Bhat", "Kapoor", "Ansari", "Singh", "Mehta", "Qureshi"][index])
        .bind(format!("candidate{index}@example.com"))
        .bind(format!("+91990000{:04}", index))
        .bind(requisitions[index % requisitions.len()])
        .bind(["applied", "screening", "interview", "offer", "hired", "rejected"][index])
        .bind(Decimal::new(30 + index as i64, 1))
        .fetch_one(&pool)
        .await?;
        if index >= 2 {
            sqlx::query(
                r#"insert into interviews (candidate_id, requisition_id, round, scheduled_at, mode, interviewer_id, status)
                   values ($1,$2,1,now() + ($3 || ' days')::interval,'virtual',$4,$5)"#,
            )
            .bind(candidate_id)
            .bind(requisitions[index % requisitions.len()])
            .bind(index as i32)
            .bind(employees["EMP-1002"])
            .bind(if index == 2 { "scheduled" } else { "completed" })
            .execute(&pool)
            .await?;
        }
        if index >= 3 {
            sqlx::query(
                r#"insert into offers (candidate_id, requisition_id, annual_ctc, expires_on, status, accepted_on)
                   values ($1,$2,$3,current_date + 10,$4,$5)"#,
            )
            .bind(candidate_id)
            .bind(requisitions[index % requisitions.len()])
            .bind(Decimal::from(420_000 + index as i64 * 10_000))
            .bind(if index == 3 { "released" } else { "accepted" })
            .bind(if index == 3 {
                None
            } else {
                Some(today)
            })
            .execute(&pool)
            .await?;
        }
    }

    // ------------------------------------------------------------------
    // Login + notifications + team WFM sample
    // ------------------------------------------------------------------
    let salt = SaltString::generate(&mut OsRng);
    let password_hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("hashing seed password: {e}"))?
        .to_string();
    sqlx::query(
        r#"insert into users (email, name, role, password_hash, employee_id)
           values ('admin@hrms.local','Platform Administrator','super_admin',$1,null)"#,
    )
    .bind(&password_hash)
    .execute(&pool)
    .await?;

    for (index, seed) in EMPLOYEES.iter().take(6).enumerate() {
        sqlx::query(
            r#"insert into notifications (employee_id, title, body, category)
               values ($1,$2,$3,$4)"#,
        )
        .bind(employees[seed.emp_no])
        .bind(if index % 2 == 0 {
            "Payslip available"
        } else {
            "Timesheet approved"
        })
        .bind(if index % 2 == 0 {
            "Your payslip for last month has been published."
        } else {
            "Your timesheet was approved by your team leader."
        })
        .bind(if index % 2 == 0 { "payroll" } else { "time" })
        .execute(&pool)
        .await?;
    }

    println!("Seed complete.");
    println!("  Login:    admin@hrms.local / {password}");
    println!(
        "  Payroll:  {run_year}-{run_month:02} posted for {} employees (gross {total_gross}, net {total_net}, TDS {total_tds})",
        EMPLOYEES.len()
    );
    Ok(())
}
