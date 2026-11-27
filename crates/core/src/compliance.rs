//! Statutory filing outputs: EPF ECR, ESI, PT and TDS 24Q / Form 16 extracts.
//! These are pure functions over already-computed payslip data so they can be
//! unit-tested and reused by both the REST API and the payroll run.

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::domain::PayPeriod;

/// Quote a CSV field when needed (RFC 4180).
fn field(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn row(values: &[String]) -> String {
    let mut out = String::new();
    for (idx, value) in values.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        out.push_str(&field(value));
    }
    out.push('\n');
    out
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PfFilingRow {
    pub uan: String,
    pub member_name: String,
    pub gross_wages: Decimal,
    pub epf_wages: Decimal,
    pub eps_wages: Decimal,
    pub edli_wages: Decimal,
    pub employee_share: Decimal,
    pub eps_contribution: Decimal,
    pub epf_employer: Decimal,
    pub ncp_days: i32,
}

/// EPFO Electronic Challan cum Return (ECR) text file layout.
pub fn epf_ecr_csv(rows: &[PfFilingRow]) -> String {
    let mut out = row(&[
        "UAN".into(),
        "MEMBER_NAME".into(),
        "GROSS_WAGES".into(),
        "EPF_WAGES".into(),
        "EPS_WAGES".into(),
        "EDLI_WAGES".into(),
        "EPF_CONTRIBUTION".into(),
        "EPS_CONTRIBUTION".into(),
        "EPF_EPS_DIFF".into(),
        "NCP_DAYS".into(),
    ]);
    for r in rows {
        out.push_str(&row(&[
            r.uan.clone(),
            r.member_name.clone(),
            format!("{:.2}", r.gross_wages),
            format!("{:.2}", r.epf_wages),
            format!("{:.2}", r.eps_wages),
            format!("{:.2}", r.edli_wages),
            format!("{:.2}", r.employee_share),
            format!("{:.2}", r.eps_contribution),
            format!("{:.2}", r.epf_employer),
            r.ncp_days.to_string(),
        ]));
    }
    out
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EsiFilingRow {
    pub ip_number: String,
    pub ip_name: String,
    pub total_wages: Decimal,
    pub employee_contribution: Decimal,
    pub employer_contribution: Decimal,
    pub contribution_days: i32,
}

pub fn esi_csv(rows: &[EsiFilingRow]) -> String {
    let mut out = row(&[
        "IP_NUMBER".into(),
        "IP_NAME".into(),
        "TOTAL_WAGES".into(),
        "EE_CONTRIBUTION".into(),
        "ER_CONTRIBUTION".into(),
        "CONTRIBUTION_DAYS".into(),
    ]);
    for r in rows {
        out.push_str(&row(&[
            r.ip_number.clone(),
            r.ip_name.clone(),
            format!("{:.2}", r.total_wages),
            format!("{:.2}", r.employee_contribution),
            format!("{:.2}", r.employer_contribution),
            r.contribution_days.to_string(),
        ]));
    }
    out
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PtFilingRow {
    pub state: String,
    pub employee_count: i64,
    pub total_wages: Decimal,
    pub tax_collected: Decimal,
}

pub fn pt_csv(rows: &[PtFilingRow]) -> String {
    let mut out = row(&[
        "STATE".into(),
        "EMPLOYEE_COUNT".into(),
        "TOTAL_WAGES".into(),
        "PT_COLLECTED".into(),
    ]);
    for r in rows {
        out.push_str(&row(&[
            r.state.clone(),
            r.employee_count.to_string(),
            format!("{:.2}", r.total_wages),
            format!("{:.2}", r.tax_collected),
        ]));
    }
    out
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TdsQuarterlyRow {
    pub pan: String,
    pub employee_name: String,
    pub section: String,
    pub amount_paid: Decimal,
    pub tds_deducted: Decimal,
    pub deduction_date: Option<NaiveDate>,
    pub challan_no: Option<String>,
}

/// Form 24Q annexure-I style extract (one row per employee per quarter).
pub fn tds_24q_csv(rows: &[TdsQuarterlyRow]) -> String {
    let mut out = row(&[
        "PAN".into(),
        "EMPLOYEE_NAME".into(),
        "SECTION".into(),
        "AMOUNT_PAID".into(),
        "TDS_DEDUCTED".into(),
        "DEDUCTION_DATE".into(),
        "CHALLAN_NO".into(),
    ]);
    for r in rows {
        out.push_str(&row(&[
            r.pan.clone(),
            r.employee_name.clone(),
            r.section.clone(),
            format!("{:.2}", r.amount_paid),
            format!("{:.2}", r.tds_deducted),
            r.deduction_date.map(|d| d.to_string()).unwrap_or_default(),
            r.challan_no.clone().unwrap_or_default(),
        ]));
    }
    out
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Form16Data {
    pub employee_name: String,
    pub pan: String,
    pub assessment_year: String,
    pub employer_name: String,
    pub employer_tan: String,
    pub gross_salary: Decimal,
    pub standard_deduction: Decimal,
    pub chapter_via_deductions: Decimal,
    pub taxable_income: Decimal,
    pub tax_payable: Decimal,
    pub tax_deducted: Decimal,
    pub regime: String,
}

/// Form 16 Part B style key/value extract.
pub fn form16_csv(data: &Form16Data) -> String {
    let mut out = row(&["FIELD".into(), "VALUE".into()]);
    let pairs: Vec<(&str, String)> = vec![
        ("EMPLOYEE_NAME", data.employee_name.clone()),
        ("PAN", data.pan.clone()),
        ("ASSESSMENT_YEAR", data.assessment_year.clone()),
        ("EMPLOYER_NAME", data.employer_name.clone()),
        ("EMPLOYER_TAN", data.employer_tan.clone()),
        ("TAX_REGIME", data.regime.clone()),
        ("GROSS_SALARY", format!("{:.2}", data.gross_salary)),
        ("STANDARD_DEDUCTION", format!("{:.2}", data.standard_deduction)),
        (
            "CHAPTER_VIA_DEDUCTIONS",
            format!("{:.2}", data.chapter_via_deductions),
        ),
        ("TAXABLE_INCOME", format!("{:.2}", data.taxable_income)),
        ("TAX_PAYABLE", format!("{:.2}", data.tax_payable)),
        ("TAX_DEDUCTED", format!("{:.2}", data.tax_deducted)),
    ];
    for (k, v) in pairs {
        out.push_str(&row(&[k.to_string(), v]));
    }
    out
}

/// Retention deadline for a payroll period (minimum 7 years per the spec).
pub fn retention_until(period: PayPeriod, retention_years: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(period.year + retention_years as i32, period.month, 1)
        .unwrap_or_else(|| period.first_day())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

    #[test]
    fn epf_ecr_has_header_and_quotes_names() {
        let csv = epf_ecr_csv(&[PfFilingRow {
            uan: "100000000001".into(),
            member_name: "Rao, Priya".into(),
            gross_wages: Decimal::from(30000),
            epf_wages: Decimal::from(15000),
            eps_wages: Decimal::from(15000),
            edli_wages: Decimal::from(15000),
            employee_share: Decimal::from(1800),
            eps_contribution: Decimal::from(1250),
            epf_employer: Decimal::from(550),
            ncp_days: 0,
        }]);
        assert!(csv.starts_with("UAN,MEMBER_NAME"));
        assert!(csv.contains("\"Rao, Priya\""));
    }

    #[test]
    fn retention_is_seven_years() {
        let until = retention_until(PayPeriod::new(2026, 11), 7);
        assert_eq!(until.to_string(), "2033-11-01");
    }
}
