//! EPF (Employees' Provident Fund) + EPS (Employees' Pension Scheme).
//!
//! Employer's 12% is split: 8.33% of EPS wages to the pension fund, and the
//! *remainder* (not a second 3.67% computation) to EPF. When an employee's PF
//! wages exceed the statutory ceiling, the EPS diversion is still capped at
//! 8.33% of the applicable ceiling and the balance falls to EPF — the rule the
//! spec calls out for "employees contributing on actual wages above Rs.15,000".

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::config::EpfConfig;
use crate::money::{max, min, round_rupee, zero};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpfSplit {
    /// Wages on which PF is actually charged (after the ceiling).
    pub pf_wages: Decimal,
    /// 12% of PF wages — employee share.
    pub employee: Decimal,
    /// 12% of PF wages — total employer share (EPF + EPS).
    pub employer_total: Decimal,
    /// Employer share that lands in the EPF account (A/c 1).
    pub employer_epf: Decimal,
    /// Employer share diverted to EPS (A/c 10).
    pub employer_eps: Decimal,
    /// EDLI contribution (A/c 21), employer cost only.
    pub edli: Decimal,
    /// EPF administrative charges (A/c 2), employer cost only.
    pub admin_charges: Decimal,
    pub ceiling_applied: bool,
    pub ceiling: Decimal,
    pub eps_wage_cap: Decimal,
    pub applicable: bool,
}

impl EpfSplit {
    pub fn not_applicable() -> Self {
        EpfSplit {
            pf_wages: zero(),
            employee: zero(),
            employer_total: zero(),
            employer_epf: zero(),
            employer_eps: zero(),
            edli: zero(),
            admin_charges: zero(),
            ceiling_applied: false,
            ceiling: zero(),
            eps_wage_cap: zero(),
            applicable: false,
        }
    }

    /// Employer money that is a cost to the company but not part of the
    /// employee's CTC-neutral gross (EPF + EPS + EDLI + admin).
    pub fn employer_cost(&self) -> Decimal {
        self.employer_total + self.edli + self.admin_charges
    }
}

/// Compute the EPF split for one wage month.
///
/// * `basic_da` — PF-eligible wages (basic + dearness allowance), prorated for
///   the paid days of the period.
/// * `on` — the wage month (used for the configurable ceiling-hike date).
/// * `override_wage` — an employee-specific PF wage (e.g. higher wage opted
///   into PF on actuals). It is still capped by the statutory ceiling.
pub fn compute(
    cfg: &EpfConfig,
    basic_da: Decimal,
    on: NaiveDate,
    override_wage: Option<Decimal>,
    applicable: bool,
) -> EpfSplit {
    if !applicable {
        return EpfSplit::not_applicable();
    }

    let ceiling = cfg.wage_ceiling_on(on);
    let base = match override_wage {
        Some(w) => max(zero(), w),
        None => max(zero(), basic_da),
    };
    if base.is_zero() {
        return EpfSplit {
            applicable: true,
            ceiling,
            ..EpfSplit::not_applicable()
        };
    }

    let pf_wages = if cfg.apply_ceiling_to_employee {
        min(base, ceiling)
    } else {
        base
    };
    let ceiling_applied = pf_wages < base;

    let employee = round_rupee(pf_wages * cfg.employee_rate_dec());
    let employer_total = round_rupee(pf_wages * cfg.employer_rate_dec());

    // EPS diversion is capped at 8.33% of the *EPS* ceiling, never above the
    // PF wage itself (so low-wage employees divert only what they earn).
    let eps_base = min(pf_wages, cfg.eps_ceiling_on(on));
    let employer_eps = round_rupee(eps_base * cfg.eps_rate_dec());
    let employer_epf = max(zero(), employer_total - employer_eps);

    let edli = round_rupee(pf_wages * crate::money::dec(cfg.edli_rate));
    let admin_charges = round_rupee(pf_wages * crate::money::dec(cfg.admin_charge_rate));

    EpfSplit {
        pf_wages,
        employee,
        employer_total,
        employer_epf,
        employer_eps,
        edli,
        admin_charges,
        ceiling_applied,
        ceiling,
        eps_wage_cap: eps_base,
        applicable: true,
    }
}
