//! Professional Tax — levied by the State on the employee's **place of work**,
//! not on the employer's registered office (spec §2.4). Article 276(2) caps the
//! total at Rs.2,500 per person per year.
//!
//! Two statutory shapes are supported:
//! * `monthly` — a monthly slab, optionally with a February surcharge that
//!   completes the annual Rs.2,500 (Karnataka, Maharashtra, West Bengal).
//! * `half_yearly` — a half-yearly slab (Tamil Nadu), split evenly across the
//!   six months of the cycle with the rounding remainder trued-up in the last
//!   month so the half-year total is exact and the annual total <= Rs.2,500.

use rust_decimal::prelude::RoundingStrategy;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::config::{PtCycle, ProfessionalTaxConfig};
use crate::domain::{Gender, PayPeriod};
use crate::money::{dec, min, zero};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtComputation {
    pub applicable: bool,
    pub state: String,
    pub state_name: String,
    pub amount: Decimal,
    pub cycle: String,
    pub annual_cap: Decimal,
    pub note: String,
}

impl PtComputation {
    pub fn not_applicable(state: &str, note: &str, annual_cap: Decimal) -> Self {
        PtComputation {
            applicable: false,
            state: state.to_string(),
            state_name: String::new(),
            amount: zero(),
            cycle: "none".into(),
            annual_cap,
            note: note.to_string(),
        }
    }
}

fn slab_amount(state: &crate::config::PtState, gross: Decimal) -> Decimal {
    for slab in &state.slabs {
        match slab.up_to {
            Some(up_to) => {
                if gross <= dec(up_to) {
                    return dec(slab.amount);
                }
            }
            None => return dec(slab.amount),
        }
    }
    zero()
}

/// Position of `period` inside its half-yearly PT cycle (1..=6).
/// Cycles run April–September and October–March.
fn cycle_position(month: u32) -> u32 {
    match month {
        4 => 1,
        5 => 2,
        6 => 3,
        7 => 4,
        8 => 5,
        9 => 6,
        10 => 1,
        11 => 2,
        12 => 3,
        1 => 4,
        2 => 5,
        _ => 6, // March
    }
}

pub fn compute(
    cfg: &ProfessionalTaxConfig,
    state_code: &str,
    gross_monthly: Decimal,
    period: PayPeriod,
    gender: Gender,
) -> PtComputation {
    let annual_cap = dec(cfg.annual_cap);
    let Some(state) = cfg.state(state_code) else {
        return PtComputation::not_applicable(
            state_code,
            "No professional-tax table configured for this work state",
            annual_cap,
        );
    };

    let state_name = if state.name.is_empty() {
        state.code.clone()
    } else {
        state.name.clone()
    };

    // Maharashtra exempts women drawing up to Rs.25,000/month.
    if state.female_exemption_upto > 0.0
        && gender.is_female()
        && gross_monthly <= dec(state.female_exemption_upto)
    {
        return PtComputation {
            applicable: true,
            state: state.code.clone(),
            state_name,
            amount: zero(),
            cycle: "monthly".into(),
            annual_cap,
            note: format!(
                "{} exempts women drawing up to Rs.{:.0}/month",
                state.code, state.female_exemption_upto
            ),
        };
    }

    match state.cycle {
        PtCycle::Monthly => {
            let mut amount = slab_amount(state, gross_monthly);
            let mut note = "Monthly professional tax".to_string();
            if period.month == 2 && state.february_surcharge > 0.0 {
                amount += dec(state.february_surcharge);
                note = format!(
                    "Monthly professional tax incl. Rs.{:.0} February surcharge (annual Rs.{:.0})",
                    state.february_surcharge, cfg.annual_cap
                );
            }
            amount = min(amount, annual_cap);
            PtComputation {
                applicable: true,
                state: state.code.clone(),
                state_name,
                amount,
                cycle: "monthly".into(),
                annual_cap,
                note,
            }
        }
        PtCycle::HalfYearly => {
            let half = slab_amount(state, gross_monthly);
            let position = cycle_position(period.month);
            let base = (half / Decimal::from(6))
                .round_dp_with_strategy(0, RoundingStrategy::ToZero);
            // The last month of the cycle carries the remainder so that the
            // six deductions add up exactly to the statutory half-year amount.
            let amount = if position < 6 {
                base
            } else {
                half - base * Decimal::from(5)
            };
            PtComputation {
                applicable: true,
                state: state.code.clone(),
                state_name,
                amount: min(amount, annual_cap),
                cycle: "half_yearly".into(),
                annual_cap,
                note: format!(
                    "Half-yearly slab Rs.{half} split over 6 months (cycle month {position}/6)"
                ),
            }
        }
    }
}

/// Full-year professional-tax liability for reporting/filing, independent of
/// the month being processed.
pub fn annual_liability(
    cfg: &ProfessionalTaxConfig,
    state_code: &str,
    gross_monthly: Decimal,
    gender: Gender,
) -> Decimal {
    let Some(state) = cfg.state(state_code) else {
        return zero();
    };
    if state.female_exemption_upto > 0.0
        && gender.is_female()
        && gross_monthly <= dec(state.female_exemption_upto)
    {
        return zero();
    }
    let cap = dec(cfg.annual_cap);
    match state.cycle {
        PtCycle::Monthly => {
            let monthly = slab_amount(state, gross_monthly);
            min(monthly * Decimal::from(12) + dec(state.february_surcharge), cap)
        }
        PtCycle::HalfYearly => min(slab_amount(state, gross_monthly) * Decimal::from(2), cap),
    }
}
