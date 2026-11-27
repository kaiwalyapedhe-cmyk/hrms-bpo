//! SPA shell and self-service screens.

use leptos::*;
use serde_json::Value;

use crate::api::{self, SessionUser};

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
    let (tab, set_tab) = create_signal("me".to_string());

    let role = user.role.clone();
    let can_see = {
        let role = role.clone();
        move |required: &str| match required {
            "admin" => role == "super_admin",
            "supervisor" => matches!(role.as_str(), "super_admin" | "manager" | "team_lead"),
            _ => true,
        }
    };
    let nav = {
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
                {nav("My workspace", "me", set_tab)}
                {nav("Time", "time", set_tab)}
                {nav("Leave", "leave", set_tab)}
                {nav("Payslips", "pay", set_tab)}
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
                "time" => view! { <TimeEntry/> }.into_view(),
                "leave" => view! { <Leave/> }.into_view(),
                "pay" => view! { <Payslips/> }.into_view(),
                "admin" => view! { <Admin/> }.into_view(),
                _ => view! { <MyWorkspace/> }.into_view(),
            }}
        </main>
    }
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
                        <p class="amount">"₹"{fmt(number(&payslip, "net_pay"))}</p>
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
                    <dt>"Object storage"</dt><dd>{move || text(&system.get(), "object_bucket")}</dd>
                    <dt>"Employees"</dt><dd>{move || fmt(number(&system.get(), "employees"))}</dd>
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
