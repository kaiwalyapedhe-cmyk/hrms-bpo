//! SPA shell, real-time dashboard and self-service screens.

use std::collections::HashMap;

use leptos::*;
use serde_json::Value;

use crate::api::{self, SessionUser};
use crate::ws::{self, ConnState};

#[derive(Debug, Clone, PartialEq)]
struct AgentRow {
    employee_id: String,
    name: String,
    aux_code: String,
    category: String,
    colour: String,
}

#[derive(Debug, Clone, PartialEq)]
struct HistPoint {
    at: String,
    aht: f64,
    wait: f64,
}

fn text(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => "—".to_string(),
        Some(other) => other.to_string().trim_matches('"').to_string(),
    }
}

fn number(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0)
}

fn fmt(value: f64) -> String {
    if (value.fract()).abs() < 0.005 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

fn short_time(raw: &str) -> String {
    // 2026-11-26T09:15:00Z → 09:15
    raw.get(11..16).unwrap_or(raw).to_string()
}

#[component]
pub fn App() -> impl IntoView {
    api::capture_api_override();
    let (session, set_session) = create_signal(api::user());
    view! {
        {move || match session.get() {
            Some(user) => view! { <Shell user=user set_session=set_session/> }.into_view(),
            None => view! { <Login set_session=set_session/> }.into_view(),
        }}
    }
}

#[component]
fn Login(set_session: WriteSignal<Option<SessionUser>>) -> impl IntoView {
    let (email, set_email) = create_signal(String::new());
    let (password, set_password) = create_signal(String::new());
    let (error, set_error) = create_signal(Option::<String>::None);
    let (busy, set_busy) = create_signal(false);

    let submit = move |ev: web_sys::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        set_busy.set(true);
        set_error.set(None);
        let email = email.get_untracked();
        let password = password.get_untracked();
        spawn_local(async move {
            match api::login(&email, &password).await {
                Ok(user) => set_session.set(Some(user)),
                Err(err) => set_error.set(Some(err)),
            }
            set_busy.set(false);
        });
    };

    view! {
        <div class="login-wrap">
            <form class="login-card" on:submit=submit>
                <div class="brand">"Sampark HRMS"</div>
                <p class="muted">"On-premises HCM for contact centres"</p>
                <label>"Email"</label>
                <input type="email" required=true autocomplete="username"
                    prop:value=move || email.get()
                    on:input=move |ev| set_email.set(event_target_value(&ev)) />
                <label>"Password"</label>
                <input type="password" required=true autocomplete="current-password"
                    prop:value=move || password.get()
                    on:input=move |ev| set_password.set(event_target_value(&ev)) />
                {move || error.get().map(|message| view! { <p class="error">{message}</p> })}
                <button class="primary" type="submit" disabled=move || busy.get()>
                    {move || if busy.get() { "Signing in…" } else { "Sign in" } }
                </button>
                <p class="muted small">"Default: admin@hrms.local / ChangeMe#2026"</p>
            </form>
        </div>
    }
}

#[component]
fn Shell(user: SessionUser, set_session: WriteSignal<Option<SessionUser>>) -> impl IntoView {
    let (tab, set_tab) = create_signal("dashboard".to_string());
    let metrics = create_rw_signal(HashMap::<String, f64>::new());
    let agents = create_rw_signal(Vec::<AgentRow>::new());
    let history = create_rw_signal(Vec::<HistPoint>::new());
    let conn = create_rw_signal(ConnState::Connecting);

    // Initial snapshot + history, then a 60s history refresh.
    spawn_local(async move {
        load_snapshot(metrics, agents, history).await;
        loop {
            gloo_timers::future::TimeoutFuture::new(60_000).await;
            if let Ok(value) = api::get_json("/dashboard/history?hours=4").await {
                history.set(parse_history(&value));
            }
        }
    });

    if let Some(token) = api::token() {
        let on_frame = Callback::new(move |frame: Value| {
            match frame.get("type").and_then(|t| t.as_str()) {
                Some("snapshot") => {
                    merge_metrics(metrics, &frame);
                    agents.set(parse_agents(&frame));
                }
                Some("metric_delta") | Some("personal_metrics") => merge_metrics(metrics, &frame),
                Some("agent_status") => {
                    if let Some(agent) = frame.get("agent") {
                        let row = parse_agent(agent);
                        agents.update(|list| {
                            match list.iter_mut().find(|a| a.employee_id == row.employee_id) {
                                Some(existing) => *existing = row,
                                None => list.push(row),
                            }
                        });
                    }
                }
                _ => {}
            }
        });
        ws::connect(token, on_frame, conn);
    }

    let role = user.role.clone();
    let can_see = {
        let role = role.clone();
        move |required: &str| match required {
            "admin" => role == "super_admin",
            "supervisor" => matches!(role.as_str(), "super_admin" | "manager" | "team_lead"),
            "quality" => matches!(role.as_str(), "super_admin" | "qa_analyst" | "team_lead"),
            _ => true,
        }
    };
    let nav = {
        let can_see = can_see.clone();
        move |label: &'static str, key: &'static str, set_tab: WriteSignal<String>| {
            let key_owned = key.to_string();
            view! {
                <button class:active=move || tab.get() == key_owned
                    on:click=move |_| set_tab.set(key.to_string())>{label}</button>
            }
        }
    };

    let user_for_logout = user.clone();
    let logout = move |_| {
        api::clear_session();
        let _ = &user_for_logout;
        set_session.set(None);
    };

    view! {
        <header class="topbar">
            <div class="brand">"Sampark HRMS"</div>
            <nav>
                {nav("Dashboard", "dashboard", set_tab)}
                {nav("My workspace", "me", set_tab)}
                {nav("Time", "time", set_tab)}
                {nav("Leave", "leave", set_tab)}
                {nav("Payslips", "pay", set_tab)}
                {nav("Quality", "qa", set_tab)}
                {can_see("supervisor").then(|| nav("Team", "team", set_tab))}
                {can_see("admin").then(|| nav("Admin", "admin", set_tab))}
            </nav>
            <div class="who">
                <span class="role">{user.role.replace('_', " ")}</span>
                <span>{user.name.clone()}</span>
                <button class="ghost" on:click=logout>"Sign out"</button>
            </div>
        </header>

        <main>
            {move || match tab.get().as_str() {
                "me" => view! { <MyWorkspace/> }.into_view(),
                "time" => view! { <TimeEntry/> }.into_view(),
                "leave" => view! { <Leave/> }.into_view(),
                "pay" => view! { <Payslips/> }.into_view(),
                "qa" => view! { <Quality/> }.into_view(),
                "team" => view! { <TeamBoard/> }.into_view(),
                "admin" => view! { <Admin/> }.into_view(),
                _ => view! { <Dashboard metrics=metrics agents=agents history=history conn=conn/> }.into_view(),
            }}
        </main>
    }
}

async fn load_snapshot(
    metrics: RwSignal<HashMap<String, f64>>,
    agents: RwSignal<Vec<AgentRow>>,
    history: RwSignal<Vec<HistPoint>>,
) {
    if let Ok(snapshot) = api::get_json("/dashboard/snapshot").await {
        merge_metrics(metrics, &snapshot);
        agents.set(parse_agents(&snapshot));
    }
    if let Ok(value) = api::get_json("/dashboard/history?hours=4").await {
        history.set(parse_history(&value));
    }
}

fn merge_metrics(metrics: RwSignal<HashMap<String, f64>>, frame: &Value) {
    if let Some(object) = frame.get("metrics").and_then(|m| m.as_object()) {
        metrics.update(|map| {
            for (key, value) in object {
                if let Some(value) = value.as_f64() {
                    map.insert(key.clone(), value);
                }
            }
        });
    }
}

fn parse_agent(value: &Value) -> AgentRow {
    AgentRow {
        employee_id: text(value, "employee_id"),
        name: text(value, "employee_name"),
        aux_code: text(value, "aux_code"),
        category: text(value, "category"),
        colour: text(value, "colour"),
    }
}

fn parse_agents(frame: &Value) -> Vec<AgentRow> {
    frame
        .get("agents")
        .and_then(|a| a.as_array())
        .map(|rows| rows.iter().map(parse_agent).collect())
        .unwrap_or_default()
}

fn parse_history(value: &Value) -> Vec<HistPoint> {
    value
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| HistPoint {
                    at: short_time(&text(row, "captured_at")),
                    aht: number(row, "aht_15m"),
                    wait: number(row, "queue_wait"),
                })
                .collect()
        })
        .unwrap_or_default()
}

#[component]
fn Dashboard(
    metrics: RwSignal<HashMap<String, f64>>,
    agents: RwSignal<Vec<AgentRow>>,
    history: RwSignal<Vec<HistPoint>>,
    conn: RwSignal<ConnState>,
) -> impl IntoView {
    let metric = move |key: &'static str| {
        Signal::derive(move || *metrics.get().get(key).unwrap_or(&0.0))
    };
    let aht_series = Signal::derive(move || history.get().iter().map(|p| p.aht).collect::<Vec<_>>());
    let wait_series = Signal::derive(move || history.get().iter().map(|p| p.wait).collect::<Vec<_>>());
    let last_aht = move || fmt(history.get().last().map(|p| p.aht).unwrap_or(0.0));

    view! {
        <section class="page-head">
            <h1>"Real-time operations dashboard"</h1>
            <span class=move || format!("pill {}", match conn.get() {
                ConnState::Live => "live",
                ConnState::Connecting => "warn",
                ConnState::Retrying => "down",
            })>
                <span class="dot"></span>{move || conn.get().label()}
            </span>
        </section>

        <section class="kpis">
            <Kpi title="Agents clocked in" value=metric("clocked_in") suffix="" hint="Punch / clock-out events"/>
            <Kpi title="Schedule adherence" value=metric("schedule_adherence") suffix="%" hint="Aux status changes"/>
            <Kpi title="Rolling AHT (15m)" value=metric("aht_15m") suffix="s" hint="New CDR records"/>
            <Kpi title="Queue wait" value=metric("queue_wait") suffix="s" hint="Call events"/>
            <Kpi title="CSAT today" value=metric("csat_avg") suffix="/5" hint="QA submissions"/>
            <Kpi title="Occupancy" value=metric("occupancy_pct") suffix="%" hint="60s tick"/>
        </section>

        <section class="charts">
            <div class="card">
                <div class="card-head"><h2>"Average handle time"</h2><span class="muted">{last_aht}"s now"</span></div>
                <Sparkline values=aht_series stroke="#38bdf8"/>
            </div>
            <div class="card">
                <div class="card-head"><h2>"Queue wait"</h2><span class="muted">"last 4 hours"</span></div>
                <Sparkline values=wait_series stroke="#fbbf24"/>
            </div>
        </section>

        <section class="card">
            <div class="card-head">
                <h2>"Agent status"</h2>
                <span class="muted">{move || format!("{} agents", agents.get().len())}</span>
            </div>
            <div class="agent-grid">
                {move || agents.get().into_iter().map(|agent| {
                    let colour = match agent.colour.as_str() {
                        "green" => "var(--ok)",
                        "yellow" => "var(--warn)",
                        "red" => "var(--bad)",
                        _ => "var(--muted)",
                    };
                    view! {
                        <div class="agent" title=agent.category.clone()>
                            <span class="agent-dot" style=format!("background:{colour}")></span>
                            <div>
                                <div class="agent-name">{agent.name}</div>
                                <div class="muted small">{agent.aux_code}</div>
                            </div>
                        </div>
                    }
                }).collect_view()}
            </div>
        </section>
    }
}

#[component]
fn Kpi(
    title: &'static str,
    value: Signal<f64>,
    suffix: &'static str,
    hint: &'static str,
) -> impl IntoView {
    view! {
        <div class="kpi">
            <div class="kpi-title">{title}</div>
            <div class="kpi-value">{move || format!("{}{}", fmt(value.get()), suffix)}</div>
            <div class="kpi-hint">{hint}</div>
        </div>
    }
}

#[component]
fn Sparkline(
    values: Signal<Vec<f64>>,
    #[prop(default = "#38bdf8")] stroke: &'static str,
) -> impl IntoView {
    view! {
        <svg class="spark" viewBox="0 0 600 170" preserveAspectRatio="none">
            <line x1="10" y1="150" x2="590" y2="150" stroke="rgba(148,163,184,.25)" stroke-width="1"/>
            <line x1="10" y1="90" x2="590" y2="90" stroke="rgba(148,163,184,.12)" stroke-width="1"/>
            <polyline fill="none" stroke=stroke stroke-width="2.5"
                points=move || path_points(&values.get()) />
        </svg>
    }
}

fn path_points(values: &[f64]) -> String {
    if values.is_empty() {
        return String::new();
    }
    let max = values.iter().cloned().fold(f64::MIN, f64::max);
    let min = values.iter().cloned().fold(f64::MAX, f64::min);
    let span = if (max - min).abs() < f64::EPSILON { 1.0 } else { max - min };
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let x = 10.0 + 580.0 * index as f64 / (values.len().max(2) - 1) as f64;
            let y = 150.0 - ((value - min) / span) * 130.0;
            format!("{x:.1},{y:.1}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[component]
fn MyWorkspace() -> impl IntoView {
    let summary = create_rw_signal(Value::Null);
    let (error, set_error) = create_signal(Option::<String>::None);
    spawn_local(async move {
        match api::get_json("/me/summary").await {
            Ok(value) => summary.set(value),
            Err(err) => set_error.set(Some(err)),
        }
    });

    view! {
        <section class="page-head"><h1>"My workspace"</h1></section>
        {move || error.get().map(|message| view! { <p class="error">{message}</p> })}
        {move || {
            let data = summary.get();
            let employee = data.get("employee").cloned().unwrap_or(Value::Null);
            let shift = data.get("today_shift").cloned().unwrap_or(Value::Null);
            let payslip = data.get("latest_payslip").cloned().unwrap_or(Value::Null);
            let balances = data.get("leave_balances").and_then(|b| b.as_array()).cloned().unwrap_or_default();
            view! {
                <section class="cards">
                    <div class="card">
                        <h2>{text(&employee, "name")}</h2>
                        <p class="muted">{text(&employee, "designation")}" · "{text(&employee, "department")}</p>
                        <dl>
                            <dt>"Employee no"</dt><dd>{text(&employee, "emp_no")}</dd>
                            <dt>"Work state"</dt><dd>{text(&employee, "work_state")}</dd>
                            <dt>"Tax regime"</dt><dd>{text(&employee, "tax_regime")}</dd>
                            <dt>"UAN"</dt><dd>{text(&employee, "uan")}</dd>
                            <dt>"Manager"</dt><dd>{text(&employee, "manager")}</dd>
                        </dl>
                    </div>
                    <div class="card">
                        <h2>"Today's shift"</h2>
                        <p>{text(&shift, "shift")}</p>
                        <p class="muted">{text(&shift, "start_time")}" – "{text(&shift, "end_time")}</p>
                        <h2 class="mt">"Latest payslip"</h2>
                        <p class="kpi-value">"₹"{fmt(number(&payslip, "net_pay"))}</p>
                        <p class="muted small">{format!("{} / {}", number(&payslip, "period_month"), number(&payslip, "period_year"))}" · net pay"</p>
                    </div>
                </section>
                <section class="card">
                    <h2>"Leave balances"</h2>
                    <table>
                        <thead><tr><th>"Type"</th><th>"Accrued"</th><th>"Used"</th><th>"Available"</th></tr></thead>
                        <tbody>
                            {balances.into_iter().map(|row| view! {
                                <tr>
                                    <td>{text(&row, "leave_name")}</td>
                                    <td>{fmt(number(&row, "accrued"))}</td>
                                    <td>{fmt(number(&row, "used"))}</td>
                                    <td><strong>{fmt(number(&row, "available"))}</strong></td>
                                </tr>
                            }).collect_view()}
                        </tbody>
                    </table>
                </section>
            }
        }}
    }
}

#[component]
fn TimeEntry() -> impl IntoView {
    let rows = create_rw_signal(Vec::<Value>::new());
    let (message, set_message) = create_signal(Option::<String>::None);

    let load = move || {
        spawn_local(async move {
            if let Ok(value) = api::get_json("/punches").await {
                rows.set(value.as_array().cloned().unwrap_or_default());
            }
        });
    };
    load();

    let punch = move |kind: &'static str| {
        spawn_local(async move {
            match api::post_json("/punches", &serde_json::json!({ "punch_type": kind })).await {
                Ok(_) => set_message.set(Some(format!("Punched {kind}"))),
                Err(err) => set_message.set(Some(err)),
            }
            if let Ok(value) = api::get_json("/punches").await {
                rows.set(value.as_array().cloned().unwrap_or_default());
            }
        });
    };

    view! {
        <section class="page-head"><h1>"Time & attendance"</h1></section>
        <section class="card">
            <div class="row">
                <button class="primary" on:click=move |_| punch("in")>"Punch in"</button>
                <button on:click=move |_| punch("break_start")>"Break start"</button>
                <button on:click=move |_| punch("break_end")>"Break end"</button>
                <button on:click=move |_| punch("out")>"Punch out"</button>
            </div>
            {move || message.get().map(|m| view! { <p class="muted">{m}</p> })}
        </section>
        <section class="card">
            <h2>"Recent punches"</h2>
            <table>
                <thead><tr><th>"Employee"</th><th>"Time"</th><th>"Type"</th><th>"Source"</th></tr></thead>
                <tbody>
                    {move || rows.get().into_iter().map(|row| view! {
                        <tr>
                            <td>{text(&row, "employee_name")}</td>
                            <td>{text(&row, "punch_time")}</td>
                            <td>{text(&row, "punch_type")}</td>
                            <td>{text(&row, "source")}</td>
                        </tr>
                    }).collect_view()}
                </tbody>
            </table>
        </section>
    }
}

#[component]
fn Leave() -> impl IntoView {
    let balances = create_rw_signal(Vec::<Value>::new());
    let requests = create_rw_signal(Vec::<Value>::new());
    let (code, set_code) = create_signal("EL".to_string());
    let (from, set_from) = create_signal(String::new());
    let (to, set_to) = create_signal(String::new());
    let (reason, set_reason) = create_signal(String::new());
    let (message, set_message) = create_signal(Option::<String>::None);

    let reload = move || {
        spawn_local(async move {
            if let Ok(value) = api::get_json("/leave/balances").await {
                balances.set(value.as_array().cloned().unwrap_or_default());
            }
            if let Ok(value) = api::get_json("/leave/requests").await {
                requests.set(value.as_array().cloned().unwrap_or_default());
            }
        });
    };
    reload();

    let submit = move |ev: web_sys::SubmitEvent| {
        ev.prevent_default();
        let payload = serde_json::json!({
            "leave_code": code.get_untracked(),
            "from_date": from.get_untracked(),
            "to_date": to.get_untracked(),
            "reason": reason.get_untracked(),
        });
        spawn_local(async move {
            match api::post_json("/leave/requests", &payload).await {
                Ok(_) => {
                    set_message.set(Some("Leave request submitted".into()));
                    if let Ok(value) = api::get_json("/leave/requests").await {
                        requests.set(value.as_array().cloned().unwrap_or_default());
                    }
                }
                Err(err) => set_message.set(Some(err)),
            }
        });
    };

    view! {
        <section class="page-head"><h1>"Absence management"</h1></section>
        <section class="cards">
            <div class="card">
                <h2>"Balances"</h2>
                <table>
                    <thead><tr><th>"Type"</th><th>"Accrued"</th><th>"Used"</th><th>"Available"</th></tr></thead>
                    <tbody>
                        {move || balances.get().into_iter().map(|row| view! {
                            <tr>
                                <td>{text(&row, "leave_name")}</td>
                                <td>{fmt(number(&row, "accrued"))}</td>
                                <td>{fmt(number(&row, "used"))}</td>
                                <td><strong>{fmt(number(&row, "available"))}</strong></td>
                            </tr>
                        }).collect_view()}
                    </tbody>
                </table>
            </div>
            <form class="card" on:submit=submit>
                <h2>"Apply for leave"</h2>
                <label>"Type"</label>
                <select on:change=move |ev| set_code.set(event_target_value(&ev))>
                    <option value="EL">"Earned leave"</option>
                    <option value="SL">"Sick leave"</option>
                    <option value="CL">"Casual leave"</option>
                </select>
                <label>"From"</label>
                <input type="date" required=true on:change=move |ev| set_from.set(event_target_value(&ev))/>
                <label>"To"</label>
                <input type="date" required=true on:change=move |ev| set_to.set(event_target_value(&ev))/>
                <label>"Reason"</label>
                <input type="text" on:input=move |ev| set_reason.set(event_target_value(&ev))/>
                <button class="primary" type="submit">"Submit"</button>
                {move || message.get().map(|m| view! { <p class="muted small">{m}</p> })}
            </form>
        </section>
        <section class="card">
            <h2>"My requests"</h2>
            <table>
                <thead><tr><th>"Type"</th><th>"From"</th><th>"To"</th><th>"Days"</th><th>"Status"</th></tr></thead>
                <tbody>
                    {move || requests.get().into_iter().map(|row| view! {
                        <tr>
                            <td>{text(&row, "leave_code")}</td>
                            <td>{text(&row, "from_date")}</td>
                            <td>{text(&row, "to_date")}</td>
                            <td>{fmt(number(&row, "days"))}</td>
                            <td><span class="pill">{text(&row, "status")}</span></td>
                        </tr>
                    }).collect_view()}
                </tbody>
            </table>
        </section>
    }
}

#[component]
fn Payslips() -> impl IntoView {
    let rows = create_rw_signal(Vec::<Value>::new());
    spawn_local(async move {
        if let Ok(value) = api::get_json("/payroll/payslips").await {
            rows.set(value.as_array().cloned().unwrap_or_default());
        }
    });
    view! {
        <section class="page-head"><h1>"Payslips"</h1></section>
        <section class="card">
            <table>
                <thead><tr>
                    <th>"Employee"</th><th>"Period"</th><th>"Paid days"</th><th>"Gross"</th>
                    <th>"Deductions"</th><th>"PT"</th><th>"TDS"</th><th>"Net pay"</th>
                </tr></thead>
                <tbody>
                    {move || rows.get().into_iter().map(|row| view! {
                        <tr>
                            <td>{text(&row, "employee_name")}</td>
                            <td>{format!("{}-{:02}", number(&row, "period_year") as i32, number(&row, "period_month") as i32)}</td>
                            <td>{fmt(number(&row, "paid_days"))}</td>
                            <td>"₹"{fmt(number(&row, "gross"))}</td>
                            <td>"₹"{fmt(number(&row, "total_deductions"))}</td>
                            <td>"₹"{fmt(number(&row, "pt_amount"))}</td>
                            <td>"₹"{fmt(number(&row, "tds"))}</td>
                            <td><strong>"₹"{fmt(number(&row, "net_pay"))}</strong></td>
                        </tr>
                    }).collect_view()}
                </tbody>
            </table>
        </section>
    }
}

#[component]
fn Quality() -> impl IntoView {
    let rows = create_rw_signal(Vec::<Value>::new());
    spawn_local(async move {
        if let Ok(value) = api::get_json("/quality/scores").await {
            rows.set(value.as_array().cloned().unwrap_or_default());
        }
    });
    view! {
        <section class="page-head"><h1>"Quality scorecard"</h1></section>
        <section class="card">
            <table>
                <thead><tr><th>"Agent"</th><th>"Evaluator"</th><th>"Form"</th><th>"Score"</th><th>"CSAT"</th><th>"When"</th></tr></thead>
                <tbody>
                    {move || rows.get().into_iter().map(|row| view! {
                        <tr>
                            <td>{text(&row, "employee_name")}</td>
                            <td>{text(&row, "evaluator_name")}</td>
                            <td>{text(&row, "form_name")}</td>
                            <td><strong>{fmt(number(&row, "weighted_pct"))}"%"</strong></td>
                            <td>{fmt(number(&row, "csat_score"))}</td>
                            <td>{text(&row, "evaluated_at")}</td>
                        </tr>
                    }).collect_view()}
                </tbody>
            </table>
        </section>
    }
}

#[component]
fn TeamBoard() -> impl IntoView {
    let rows = create_rw_signal(Vec::<Value>::new());
    let overview = create_rw_signal(Value::Null);
    spawn_local(async move {
        if let Ok(value) = api::get_json("/dashboard/team").await {
            rows.set(value.as_array().cloned().unwrap_or_default());
        }
        if let Ok(value) = api::get_json("/manager/overview").await {
            overview.set(value);
        }
    });
    view! {
        <section class="page-head"><h1>"Team performance"</h1></section>
        <section class="kpis">
            <div class="kpi"><div class="kpi-title">"Team size"</div><div class="kpi-value">{move || fmt(number(&overview.get(), "team_size"))}</div></div>
            <div class="kpi"><div class="kpi-title">"Present today"</div><div class="kpi-value">{move || fmt(number(&overview.get(), "present_today"))}</div></div>
            <div class="kpi"><div class="kpi-title">"Pending timesheets"</div><div class="kpi-value">{move || fmt(number(&overview.get(), "pending_timesheets"))}</div></div>
            <div class="kpi"><div class="kpi-title">"Pending leave"</div><div class="kpi-value">{move || fmt(number(&overview.get(), "pending_leave"))}</div></div>
            <div class="kpi"><div class="kpi-title">"Calls today"</div><div class="kpi-value">{move || fmt(number(&overview.get(), "calls_today"))}</div></div>
        </section>
        <section class="card">
            <table>
                <thead><tr><th>"Agent"</th><th>"Ext"</th><th>"Aux"</th><th>"Calls"</th><th>"Avg handle"</th><th>"QA"</th><th>"CSAT"</th></tr></thead>
                <tbody>
                    {move || rows.get().into_iter().map(|row| view! {
                        <tr>
                            <td>{text(&row, "employee_name")}</td>
                            <td>{text(&row, "extension")}</td>
                            <td>{text(&row, "aux_code")}</td>
                            <td>{fmt(number(&row, "calls_today"))}</td>
                            <td>{fmt(number(&row, "avg_handle_seconds"))}"s"</td>
                            <td>{fmt(number(&row, "qa_score"))}</td>
                            <td>{fmt(number(&row, "csat"))}</td>
                        </tr>
                    }).collect_view()}
                </tbody>
            </table>
        </section>
    }
}

#[component]
fn Admin() -> impl IntoView {
    let system = create_rw_signal(Value::Null);
    let users = create_rw_signal(Vec::<Value>::new());
    let audit = create_rw_signal(Vec::<Value>::new());
    spawn_local(async move {
        if let Ok(value) = api::get_json("/admin/system").await {
            system.set(value);
        }
        if let Ok(value) = api::get_json("/admin/users").await {
            users.set(value.as_array().cloned().unwrap_or_default());
        }
        if let Ok(value) = api::get_json("/admin/audit?limit=50").await {
            audit.set(value.as_array().cloned().unwrap_or_default());
        }
    });
    view! {
        <section class="page-head"><h1>"Platform administration"</h1></section>
        <section class="cards">
            <div class="card">
                <h2>"System"</h2>
                <dl>
                    <dt>"Version"</dt><dd>{move || text(&system.get(), "version")}</dd>
                    <dt>"EPF ceiling"</dt><dd>"₹"{move || fmt(number(&system.get(), "epf_ceiling"))}</dd>
                    <dt>"ESI ceiling"</dt><dd>"₹"{move || fmt(number(&system.get(), "esi_ceiling"))}</dd>
                    <dt>"Redis queue"</dt><dd>{move || if system.get().get("redis_queue_enabled").and_then(|v| v.as_bool()).unwrap_or(false) { "enabled" } else { "disabled" } }</dd>
                    <dt>"Object storage"</dt><dd>{move || text(&system.get(), "object_bucket")}</dd>
                    <dt>"Employees"</dt><dd>{move || fmt(number(&system.get(), "employees"))}</dd>
                    <dt>"Dashboard clients"</dt><dd>{move || fmt(number(&system.get(), "dashboard_subscribers"))}</dd>
                </dl>
            </div>
            <div class="card">
                <h2>"User accounts"</h2>
                <table>
                    <thead><tr><th>"Email"</th><th>"Role"</th><th>"Active"</th></tr></thead>
                    <tbody>
                        {move || users.get().into_iter().map(|row| view! {
                            <tr>
                                <td>{text(&row, "email")}</td>
                                <td>{text(&row, "role")}</td>
                                <td>{text(&row, "active")}</td>
                            </tr>
                        }).collect_view()}
                    </tbody>
                </table>
            </div>
        </section>
        <section class="card">
            <h2>"Audit trail"</h2>
            <table>
                <thead><tr><th>"When"</th><th>"Actor"</th><th>"Action"</th><th>"Entity"</th></tr></thead>
                <tbody>
                    {move || audit.get().into_iter().map(|row| view! {
                        <tr>
                            <td>{text(&row, "created_at")}</td>
                            <td>{text(&row, "actor_email")}</td>
                            <td>{text(&row, "action")}</td>
                            <td>{text(&row, "entity")}</td>
                        </tr>
                    }).collect_view()}
                </tbody>
            </table>
        </section>
    }
}
