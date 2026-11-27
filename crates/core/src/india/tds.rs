//! Salary TDS — Income-tax Act, s.115BAC (new regime) by default, with opt-out
//! to the old regime.
//!
//! Implemented rules:
//! * FY 2025-26 new-regime slabs (0–4L nil … above 24L 30%).
//! * Standard deduction Rs.75,000 (new) / Rs.50,000 (old).
//! * s.87A rebate: new regime up to Rs.60,000 where taxable income <= Rs.12L
//!   (so a salaried employee at Rs.12.75L gross pays nil); old regime
//!   Rs.12,500 where taxable income <= Rs.5L.
//! * Marginal relief just above the s.87A threshold, so tax never exceeds the
//!   income above Rs.12L.
//! * Surcharge slabs (capped at 25%) and 4% health & education cess.
//! * Old-regime Chapter VI-A caps (80C/80D/80CCD(1B)/home-loan interest) and
//!   the s.10(13A) HRA exemption.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::config::TdsConfig;
use crate::domain::TaxRegime;
use crate::money::{dec, max, min, round_rupee, zero};

/// Employee investment/rent declarations used for the old regime and for HRA.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaxDeclarations {
    #[serde(default)]
    pub section_80c: Decimal,
    #[serde(default)]
    pub section_80d: Decimal,
    #[serde(default)]
    pub section_80ccd_1b: Decimal,
    #[serde(default)]
    pub home_loan_interest: Decimal,
    #[serde(default)]
    pub hra_rent_paid_annual: Decimal,
    #[serde(default)]
    pub hra_received_annual: Decimal,
    #[serde(default)]
    pub basic_da_annual: Decimal,
    #[serde(default)]
    pub metro_city: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaxBreakdown {
    pub regime: String,
    pub gross_annual: Decimal,
    pub standard_deduction: Decimal,
    /// Chapter VI-A + HRA exemption actually allowed.
    pub other_deductions: Decimal,
    pub hra_exemption: Decimal,
    pub taxable_income: Decimal,
    pub slab_tax: Decimal,
    pub rebate_87a: Decimal,
    pub surcharge: Decimal,
    pub cess: Decimal,
    pub total_tax: Decimal,
    pub effective_rate_pct: Decimal,
    pub notes: Vec<String>,
}

fn slab_tax(slabs: &[crate::config::TaxSlab], taxable: Decimal) -> Decimal {
    let mut tax = zero();
    let mut previous = zero();
    for slab in slabs {
        if taxable <= previous {
            break;
        }
        let upper = match slab.up_to {
            Some(up_to) => dec(up_to),
            None => taxable,
        };
        let portion = min(taxable, upper) - previous;
        if portion > zero() {
            tax += portion * dec(slab.rate);
        }
        previous = upper;
    }
    tax
}

fn surcharge_rate(slabs: &[crate::config::TaxSlab], taxable: Decimal) -> Decimal {
    for slab in slabs {
        match slab.up_to {
            Some(up_to) => {
                if taxable <= dec(up_to) {
                    return dec(slab.rate);
                }
            }
            None => return dec(slab.rate),
        }
    }
    zero()
}

fn hra_exemption(cfg: &TdsConfig, d: &TaxDeclarations) -> Decimal {
    if !cfg.hra_exemption_enabled || d.hra_rent_paid_annual <= zero() || d.hra_received_annual <= zero()
    {
        return zero();
    }
    let salary = max(d.basic_da_annual, zero());
    if salary.is_zero() {
        return zero();
    }
    let rent_minus_ten_pct = max(d.hra_rent_paid_annual - salary * dec(0.10), zero());
    let percent = if d.metro_city { dec(0.50) } else { dec(0.40) };
    let salary_limit = salary * percent;
    min(
        min(d.hra_received_annual, rent_minus_ten_pct),
        salary_limit,
    )
}

/// Annual tax liability on a projected annual gross salary.
pub fn annual_tax(
    cfg: &TdsConfig,
    regime: TaxRegime,
    gross_annual: Decimal,
    declarations: &TaxDeclarations,
) -> TaxBreakdown {
    let gross_annual = max(gross_annual, zero());
    let mut notes = Vec::new();

    let (standard_deduction, slabs, rebate_limit, rebate_max) = match regime {
        TaxRegime::New => (
            dec(cfg.standard_deduction_new),
            &cfg.new_regime_slabs,
            dec(cfg.rebate_new_income_limit),
            dec(cfg.rebate_new_max),
        ),
        TaxRegime::Old => (
            dec(cfg.standard_deduction_old),
            &cfg.old_regime_slabs,
            dec(cfg.rebate_old_income_limit),
            dec(cfg.rebate_old_max),
        ),
    };

    let mut other_deductions = zero();
    let mut hra = zero();
    if regime == TaxRegime::Old {
        let c80 = min(max(declarations.section_80c, zero()), dec(cfg.cap_80c));
        let c80d = min(max(declarations.section_80d, zero()), dec(cfg.cap_80d));
        let nps = min(
            max(declarations.section_80ccd_1b, zero()),
            dec(cfg.cap_80ccd_1b),
        );
        let home = min(
            max(declarations.home_loan_interest, zero()),
            dec(cfg.cap_home_loan_interest),
        );
        hra = hra_exemption(cfg, declarations);
        other_deductions = c80 + c80d + nps + home + hra;
        notes.push(format!(
            "Old regime deductions: 80C {c80}, 80D {c80d}, 80CCD(1B) {nps}, home loan {home}, HRA {hra}"
        ));
    }

    let taxable_income = max(
        gross_annual - standard_deduction - other_deductions,
        zero(),
    );

    let raw_tax = slab_tax(slabs, taxable_income);

    let mut rebate = zero();
    if taxable_income <= rebate_limit && raw_tax > zero() {
        rebate = min(raw_tax, rebate_max);
        if rebate > zero() {
            notes.push(format!(
                "s.87A rebate applied: taxable income {taxable_income} <= {rebate_limit}"
            ));
        }
    }

    let mut tax_after_rebate = max(raw_tax - rebate, zero());

    // Marginal relief above the s.87A threshold (new regime): the tax payable
    // is capped at the amount by which income exceeds Rs.12L.
    if regime == TaxRegime::New && cfg.marginal_relief && taxable_income > rebate_limit {
        let excess = taxable_income - rebate_limit;
        if tax_after_rebate > excess {
            let relief = tax_after_rebate - excess;
            tax_after_rebate = excess;
            notes.push(format!("Marginal relief under s.87A: {relief}"));
        }
    }

    let surcharge = round_rupee(tax_after_rebate * surcharge_rate(&cfg.surcharge_slabs, taxable_income));
    if surcharge > zero() {
        notes.push("Surcharge applied".into());
    }
    let cess = round_rupee((tax_after_rebate + surcharge) * dec(cfg.cess_rate));
    let total_tax = round_rupee(tax_after_rebate + surcharge + cess);

    let effective_rate_pct = if gross_annual > zero() {
        (total_tax / gross_annual * Decimal::from(100)).round_dp(2)
    } else {
        zero()
    };

    TaxBreakdown {
        regime: regime.as_str().to_string(),
        gross_annual,
        standard_deduction,
        other_deductions,
        hra_exemption: hra,
        taxable_income,
        slab_tax: round_rupee(raw_tax),
        rebate_87a: round_rupee(rebate),
        surcharge,
        cess,
        total_tax,
        effective_rate_pct,
        notes,
    }
}

/// Spread the remaining annual liability over the remaining months of the
/// financial year, net of TDS already deducted.
pub fn monthly_tds(annual_liability: Decimal, ytd_tds: Decimal, months_remaining: u8) -> Decimal {
    let remaining = max(annual_liability - max(ytd_tds, zero()), zero());
    if remaining.is_zero() {
        return zero();
    }
    let months = max(Decimal::from(months_remaining.max(1)), Decimal::ONE);
    min(round_rupee(remaining / months), remaining)
}
