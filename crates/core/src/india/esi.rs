//! ESI — employee 0.75%, employer 3.25%, ceiling Rs.21,000 (Rs.25,000 for
//! employees with disability). Every fractional rupee is rounded **up**, which
//! is why `esi_at_exactly_ceiling` in the integration tests asserts 158 and 683
//! rather than 157.50 / 682.50.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::config::EsiConfig;
use crate::money::{dec, round_rupee, round_rupee_up, zero};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EsiContribution {
    pub applicable: bool,
    pub wages: Decimal,
    pub ceiling: Decimal,
    pub employee: Decimal,
    pub employer: Decimal,
}

impl EsiContribution {
    pub fn not_applicable(wages: Decimal, ceiling: Decimal) -> Self {
        EsiContribution {
            applicable: false,
            wages,
            ceiling,
            employee: zero(),
            employer: zero(),
        }
    }

    pub fn employer_cost(&self) -> Decimal {
        self.employer
    }
}

/// ESI is charged on gross wages (excluding reimbursements) only while the
/// employee is within the wage ceiling for the contribution period.
pub fn compute(cfg: &EsiConfig, wages: Decimal, disability: bool, applicable: bool) -> EsiContribution {
    let ceiling = cfg.ceiling_for(disability);
    if !applicable || wages <= zero() {
        return EsiContribution::not_applicable(wages, ceiling);
    }
    if wages > ceiling {
        return EsiContribution::not_applicable(wages, ceiling);
    }

    let round = |v: Decimal| {
        if cfg.round_up_to_rupee {
            round_rupee_up(v)
        } else {
            round_rupee(v)
        }
    };

    EsiContribution {
        applicable: true,
        wages,
        ceiling,
        employee: round(wages * dec(cfg.employee_rate)),
        employer: round(wages * dec(cfg.employer_rate)),
    }
}
