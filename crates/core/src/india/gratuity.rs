//! Gratuity — Payment of Gratuity Act 1972.
//!
//! Formula: `(15 x last drawn wages x completed years of service) / 26`, wages
//! being basic + dearness allowance. Private-sector ceiling Rs.20,00,000.
//! Eligibility: 5 years continuous service, waived on death/disablement, and
//! 1 year for fixed-term employees.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::config::GratuityConfig;
use crate::domain::{EmploymentType, SeparationReason};
use crate::money::{dec, max, min, round2, zero};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GratuityResult {
    pub eligible: bool,
    pub years_considered: u32,
    pub wages: Decimal,
    pub gross_gratuity: Decimal,
    pub payable: Decimal,
    pub capped: bool,
    pub reason: String,
}

/// Full-and-final gratuity settlement.
///
/// * `years` — completed years of continuous service.
/// * `months_in_final_year` — completed months beyond `years`; >= 6 rounds up.
pub fn settlement(
    cfg: &GratuityConfig,
    monthly_wages: Decimal,
    years: u32,
    months_in_final_year: u32,
    employment_type: EmploymentType,
    reason: SeparationReason,
) -> GratuityResult {
    let wages = max(monthly_wages, zero());
    let waived = matches!(
        reason,
        SeparationReason::Death | SeparationReason::Disablement
    );

    let required_years = match employment_type {
        EmploymentType::FixedTerm => cfg.fixed_term_min_years,
        _ => cfg.min_service_years,
    };

    let mut years_considered = years;
    if months_in_final_year >= cfg.round_partial_year_months {
        years_considered += 1;
    }

    let eligible = waived || years_considered >= required_years;

    if !eligible {
        return GratuityResult {
            eligible: false,
            years_considered,
            wages,
            gross_gratuity: zero(),
            payable: zero(),
            capped: false,
            reason: format!(
                "Not eligible: {years_considered} year(s) of service, {required_years} required ({}).",
                employment_type.as_str()
            ),
        };
    }

    let gross = round2(
        dec(cfg.days_numerator) * wages * Decimal::from(years_considered)
            / dec(cfg.divisor_days),
    );
    let cap = dec(cfg.wage_cap);
    let payable = min(gross, cap);

    GratuityResult {
        eligible: true,
        years_considered,
        wages,
        gross_gratuity: gross,
        payable,
        capped: gross > cap,
        reason: if waived {
            "Eligibility waived on death/disablement (s.4(1) proviso)".into()
        } else {
            format!("{years_considered} completed year(s) of continuous service")
        },
    }
}

/// Monthly provision the payroll engine books while the employee is in
/// service: one twelfth of the accrual for the year.
pub fn monthly_accrual(cfg: &GratuityConfig, monthly_wages: Decimal) -> Decimal {
    let wages = max(monthly_wages, zero());
    round2(dec(cfg.days_numerator) * wages / dec(cfg.divisor_days) / Decimal::from(12))
}
