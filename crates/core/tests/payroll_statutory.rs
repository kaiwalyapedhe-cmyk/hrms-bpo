//! Statutory integration tests required by the delivery spec:
//! EPF 12%/3.67%/8.33% split, ESI rounding above the Rs.21,000 ceiling,
//! PT by work state, TDS nil liability at Rs.12L (new regime) and gratuity
//! eligibility — plus end-to-end payslip checks through the engine.

use hrms_core::domain::{EmploymentType, Gender, PayPeriod, SeparationReason, TaxRegime};
use hrms_core::india::tds::TaxDeclarations;
use hrms_core::{IndiaPayrollConfig, PayrollEngine, PayrollInput};
use rust_decimal::Decimal;
use std::str::FromStr;
use uuid::Uuid;

fn d(value: &str) -> Decimal {
    Decimal::from_str(value).expect("decimal")
}

fn shipped_config() -> IndiaPayrollConfig {
    IndiaPayrollConfig::load_from_path("../../config/india_payroll.toml")
        .expect("shipped config/india_payroll.toml parses")
}

fn engine() -> PayrollEngine {
    PayrollEngine::new(shipped_config())
}

fn base_input(basic: &str) -> PayrollInput {
    let period = PayPeriod::new(2026, 11);
    let mut input = PayrollInput::new(Uuid::new_v4(), period);
    input.basic = d(basic);
    input.work_state = "KA".into();
    input.tax_regime = TaxRegime::New;
    input.service_years = 6;
    input
}

// ---------------------------------------------------------------------------
// EPF
// ---------------------------------------------------------------------------

#[test]
fn epf_split_at_ceiling_is_12_367_833() {
    // Basic 20,000 → PF wages capped at 15,000.
    let slip = engine().compute(&base_input("20000")).unwrap();
    assert_eq!(slip.pf.pf_wages, d("15000"));
    assert_eq!(slip.pf.employee, d("1800"), "12% of 15,000");
    assert_eq!(slip.pf.employer_total, d("1800"), "employer 12%");
    assert_eq!(slip.pf.employer_eps, d("1250"), "8.33% of 15,000 = 1249.5 → 1250");
    assert_eq!(slip.pf.employer_epf, d("550"), "remainder of 12% goes to EPF");
    assert_eq!(
        slip.pf.employer_epf + slip.pf.employer_eps,
        slip.pf.employer_total
    );
    assert!(slip.pf.ceiling_applied);
    assert!(slip.message_codes().contains(&"PF_CEILING_APPLIED".to_string()));
}

#[test]
fn epf_split_below_ceiling_uses_actual_wages() {
    let slip = engine().compute(&base_input("12000")).unwrap();
    assert_eq!(slip.pf.pf_wages, d("12000"));
    assert_eq!(slip.pf.employee, d("1440"));
    assert_eq!(slip.pf.employer_eps, d("1000"), "999.6 rounds to 1000");
    assert_eq!(slip.pf.employer_epf, d("440"));
    assert!(!slip.pf.ceiling_applied);
}

#[test]
fn epf_eps_diversion_capped_at_eps_ceiling_above_rs25000() {
    // Contributing on actual wages once the Rs.25,000 ceiling hike is enabled.
    let mut cfg = shipped_config();
    cfg.epf.ceiling_hike.enabled = true;
    let engine = PayrollEngine::new(cfg);

    let slip = engine.compute(&base_input("30000")).unwrap();
    assert_eq!(slip.pf.pf_wages, d("25000"), "new ceiling applies after 17-Sep-2026");
    assert_eq!(slip.pf.employee, d("3000"));
    assert_eq!(slip.pf.employer_eps, d("2083"), "8.33% of 25,000 = 2082.5 → 2083");
    assert_eq!(slip.pf.employer_epf, d("917"), "3000 - 2083, remainder to EPF");
}

#[test]
fn epf_skipped_for_exempt_employee() {
    let mut input = base_input("20000");
    input.pf_applicable = false;
    let slip = engine().compute(&input).unwrap();
    assert_eq!(slip.pf.employee, Decimal::ZERO);
    assert_eq!(slip.pf.employer_total, Decimal::ZERO);
}

// ---------------------------------------------------------------------------
// ESI
// ---------------------------------------------------------------------------

#[test]
fn esi_rounds_fractions_up_at_the_ceiling() {
    let slip = engine().compute(&base_input("21000")).unwrap();
    assert!(slip.esi.applicable);
    assert_eq!(slip.esi.wages, d("21000"));
    assert_eq!(slip.esi.employee, d("158"), "0.75% of 21,000 = 157.50 → 158");
    assert_eq!(slip.esi.employer, d("683"), "3.25% of 21,000 = 682.50 → 683");
}

#[test]
fn esi_stops_above_the_ceiling() {
    let slip = engine().compute(&base_input("21001")).unwrap();
    assert!(!slip.esi.applicable);
    assert_eq!(slip.esi.employee, Decimal::ZERO);
    assert_eq!(slip.esi.employer, Decimal::ZERO);
    assert!(slip
        .message_codes()
        .contains(&"ESI_CEILING_EXCEEDED".to_string()));
}

#[test]
fn esi_disability_ceiling_is_25000() {
    let mut input = base_input("24000");
    input.disability = true;
    let slip = engine().compute(&input).unwrap();
    assert!(slip.esi.applicable);
    assert_eq!(slip.esi.ceiling, d("25000"));
    assert_eq!(slip.esi.employee, d("180"));
    assert_eq!(slip.esi.employer, d("780"));

    // The same wage without disability is outside ESI.
    let slip = engine().compute(&base_input("24000")).unwrap();
    assert!(!slip.esi.applicable);
}

// ---------------------------------------------------------------------------
// Professional tax — place of work
// ---------------------------------------------------------------------------

fn pt(engine: &PayrollEngine, state: &str, gross: &str, month: u32, gender: Gender) -> Decimal {
    engine
        .professional_tax_liability(state, d(gross), PayPeriod::new(2026, month), gender)
        .amount
}

#[test]
fn professional_tax_follows_work_state_not_employer_state() {
    let engine = engine();
    // Karnataka: > 25,000 → 200/month, 300 in February.
    assert_eq!(pt(&engine, "KA", "30000", 11, Gender::Male), d("200"));
    assert_eq!(pt(&engine, "KA", "30000", 2, Gender::Male), d("300"));
    assert_eq!(pt(&engine, "KA", "12000", 11, Gender::Male), d("0"));
    // West Bengal slabs.
    assert_eq!(pt(&engine, "WB", "20000", 11, Gender::Male), d("130"));
    assert_eq!(pt(&engine, "WB", "45000", 11, Gender::Male), d("200"));
    // Maharashtra: 175 in the 7,501–10,000 slab, and women up to 25,000 exempt.
    assert_eq!(pt(&engine, "MH", "9000", 11, Gender::Male), d("175"));
    assert_eq!(pt(&engine, "MH", "20000", 11, Gender::Female), d("0"));
    assert_eq!(pt(&engine, "MH", "30000", 11, Gender::Female), d("200"));
    // Unconfigured state → nil, with an explicit payroll message.
    let mut input = base_input("30000");
    input.work_state = "ZZ".into();
    let slip = engine.compute(&input).unwrap();
    assert_eq!(slip.professional_tax.amount, Decimal::ZERO);
    assert!(slip
        .message_codes()
        .contains(&"PT_STATE_NOT_CONFIGURED".to_string()));
}

#[test]
fn karnataka_annual_pt_adds_up_to_2500() {
    let engine = engine();
    let mut total = Decimal::ZERO;
    for month in 1..=12 {
        total += pt(&engine, "KA", "30000", month, Gender::Male);
    }
    assert_eq!(total, d("2500"));
}

#[test]
fn tamil_nadu_half_yearly_cycle_is_exact() {
    let engine = engine();
    // Rs.1,250 per half-year for wages above 75,000.
    let april = pt(&engine, "TN", "80000", 4, Gender::Male);
    let may = pt(&engine, "TN", "80000", 5, Gender::Male);
    let september = pt(&engine, "TN", "80000", 9, Gender::Male);
    assert_eq!(april, d("208"));
    assert_eq!(may, d("208"));
    assert_eq!(september, d("210"), "remainder trued-up in the last month");

    let mut half_year = Decimal::ZERO;
    for month in [4, 5, 6, 7, 8, 9] {
        half_year += pt(&engine, "TN", "80000", month, Gender::Male);
    }
    assert_eq!(half_year, d("1250"));
    assert!(half_year * Decimal::from(2) <= d("2500"));
    // Cycle restarts in October.
    assert_eq!(pt(&engine, "TN", "80000", 10, Gender::Male), d("208"));
}

#[test]
fn professional_tax_never_exceeds_article_276_cap() {
    let engine = engine();
    for month in 1..=12 {
        for state in ["MH", "KA", "WB", "TN"] {
            for gross in ["5000", "12000", "40000", "250000"] {
                let amount = pt(&engine, state, gross, month, Gender::Male);
                assert!(
                    amount <= d("2500"),
                    "{state} month {month} gross {gross} produced {amount}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TDS
// ---------------------------------------------------------------------------

#[test]
fn tds_is_nil_at_12_lakh_new_regime() {
    let engine = engine();
    let declarations = TaxDeclarations::default();
    let tax = engine.annual_tax_projection(TaxRegime::New, d("1200000"), &declarations);
    assert_eq!(tax.taxable_income, d("1125000"));
    assert_eq!(tax.total_tax, Decimal::ZERO);
}

#[test]
fn salaried_employee_at_1275000_pays_nil_under_new_regime() {
    let engine = engine();
    let declarations = TaxDeclarations::default();
    let tax = engine.annual_tax_projection(TaxRegime::New, d("1275000"), &declarations);
    assert_eq!(tax.taxable_income, d("1200000"), "75,000 standard deduction");
    assert_eq!(tax.slab_tax, d("60000"));
    assert_eq!(tax.rebate_87a, d("60000"), "s.87A rebate cancels the tax");
    assert_eq!(tax.total_tax, Decimal::ZERO);
}

#[test]
fn marginal_relief_just_above_the_rebate_threshold() {
    let engine = engine();
    let tax = engine.annual_tax_projection(TaxRegime::New, d("1300000"), &TaxDeclarations::default());
    assert_eq!(tax.taxable_income, d("1225000"));
    assert_eq!(tax.total_tax, d("26000"), "tax capped at income above 12L + 4% cess");
}

#[test]
fn old_regime_uses_its_own_slabs_and_caps() {
    let engine = engine();
    let declarations = TaxDeclarations {
        section_80c: d("200000"), // capped at 1,50,000
        section_80d: d("25000"),
        ..Default::default()
    };
    let tax = engine.annual_tax_projection(TaxRegime::Old, d("1275000"), &declarations);
    assert_eq!(tax.standard_deduction, d("50000"));
    assert_eq!(tax.other_deductions, d("175000"));
    assert_eq!(tax.taxable_income, d("1050000"));
    assert_eq!(tax.slab_tax, d("127500"), "12,500 + 1,00,000 + 15,000");
    assert_eq!(tax.total_tax, d("132600"), "plus 4% cess");
}

#[test]
fn tds_is_spread_over_remaining_months() {
    let engine = engine();
    let mut input = base_input("250000");
    input.annual_gross_projection = Some(d("3000000"));
    input.ytd_tds = d("100000");
    let slip = engine.compute(&input).unwrap();
    // Taxable 29,25,000 → slab tax 4,57,500; + 4% cess = 4,75,800.
    let annual = slip.tds.total_tax;
    assert_eq!(annual, d("475800"));
    let remaining = annual - d("100000");
    let expected = (remaining / Decimal::from(5)).round_dp(0);
    assert_eq!(slip.tds_monthly, expected);
    assert_eq!(slip.tds_monthly, d("75160"));
}

// ---------------------------------------------------------------------------
// Gratuity
// ---------------------------------------------------------------------------

#[test]
fn gratuity_eligibility_rules() {
    let engine = engine();
    let not_eligible = engine.gratuity_settlement(
        d("30000"),
        4,
        3,
        EmploymentType::FullTime,
        SeparationReason::Resignation,
    );
    assert!(!not_eligible.eligible);
    assert_eq!(not_eligible.payable, Decimal::ZERO);

    let eligible = engine.gratuity_settlement(
        d("30000"),
        5,
        0,
        EmploymentType::FullTime,
        SeparationReason::Resignation,
    );
    assert!(eligible.eligible);
    assert_eq!(eligible.payable, d("86538.46")); // 15 × 30,000 × 5 / 26

    let rounded_up = engine.gratuity_settlement(
        d("30000"),
        4,
        7,
        EmploymentType::FullTime,
        SeparationReason::Resignation,
    );
    assert!(rounded_up.eligible, ">= 6 months rounds up to 5 years");

    let fixed_term = engine.gratuity_settlement(
        d("20000"),
        1,
        0,
        EmploymentType::FixedTerm,
        SeparationReason::FixedTermCompletion,
    );
    assert!(fixed_term.eligible, "1 year for fixed-term employees");

    let death = engine.gratuity_settlement(
        d("20000"),
        2,
        1,
        EmploymentType::FullTime,
        SeparationReason::Death,
    );
    assert!(death.eligible, "5-year rule waived on death");
}

#[test]
fn gratuity_is_capped_at_20_lakh() {
    let engine = engine();
    let result = engine.gratuity_settlement(
        d("200000"),
        30,
        0,
        EmploymentType::FullTime,
        SeparationReason::Retirement,
    );
    assert!(result.capped);
    assert_eq!(result.gross_gratuity, d("3461538.46"));
    assert_eq!(result.payable, d("2000000"));
}

// ---------------------------------------------------------------------------
// Whole payslip
// ---------------------------------------------------------------------------

#[test]
fn full_payslip_reconciles() {
    let mut input = base_input("15000");
    input.hra = d("6000");
    input.special_allowance = d("9000");
    input.ctc_annual = Some(d("360000"));
    let slip = engine().compute(&input).unwrap();

    assert_eq!(slip.gross_earnings, d("30000"));
    assert_eq!(slip.pf.employee, d("1800"));
    assert_eq!(slip.professional_tax.amount, d("200"));
    assert_eq!(slip.esi.employee, Decimal::ZERO, "above the ESI ceiling");
    assert_eq!(slip.tds_monthly, Decimal::ZERO, "annual projection below 12L");
    assert_eq!(slip.total_deductions, d("2000"));
    assert_eq!(slip.net_pay, d("28000"));
    assert_eq!(
        slip.employer_cost,
        d("30000") + slip.pf.employer_total + slip.pf.edli + slip.pf.admin_charges
            + slip.gratuity_accrual
    );
    assert!(slip.gratuity_eligible);
    assert!(!slip.has_errors());
}

#[test]
fn partial_period_prorates_and_flags_the_message() {
    let mut input = base_input("15000");
    input.hra = d("6000");
    input.special_allowance = d("9000");
    input.paid_days = Decimal::from(15);
    let slip = engine().compute(&input).unwrap();

    assert_eq!(slip.days_in_month, 30);
    assert_eq!(slip.lop_days, Decimal::from(15));
    assert_eq!(slip.gross_earnings, d("15000"));
    assert_eq!(slip.pf.pf_wages, d("7500"));
    assert!(slip.message_codes().contains(&"PARTIAL_PERIOD".to_string()));
}

#[test]
fn wage_50_percent_rule_is_reported() {
    let mut input = base_input("8000");
    input.special_allowance = d("22000"); // basic is 26.7% of gross
    let slip = engine().compute(&input).unwrap();
    assert!(slip.message_codes().contains(&"WAGE_50PCT_RULE".to_string()));
    assert!(slip.has_errors());
}

#[test]
fn overtime_uses_double_rate_by_default() {
    let mut input = base_input("26000");
    input.overtime_hours = d("10");
    let slip = engine().compute(&input).unwrap();
    // hourly = 26,000 / 30 / 8 = 108.33; 10 h × 2 = 2,166.67
    assert_eq!(slip.gross_earnings, d("28166.67"));
    assert!(slip.message_codes().contains(&"OVERTIME_PAID".to_string()));
}

#[test]
fn reversal_negates_the_payslip() {
    let input = base_input("15000");
    let original = engine().compute(&input).unwrap();
    let slip = engine().reversal(&input).unwrap();
    assert_eq!(slip.gross_earnings, -original.gross_earnings);
    assert_eq!(slip.net_pay, -original.net_pay);
    assert_eq!(slip.total_deductions, -original.total_deductions);
    assert!(slip.message_codes().contains(&"REVERSAL".to_string()));
}

#[test]
fn invalid_input_is_rejected() {
    let mut input = base_input("15000");
    input.paid_days = Decimal::from(31);
    assert!(engine().compute(&input).is_err());
}
