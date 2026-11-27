//! The payroll engine: turns an employee's pay data + attendance into a fully
//! costed payslip (employee deductions, employer contributions, TDS, gratuity
//! provision and payroll messages).

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::IndiaPayrollConfig;
use crate::domain::{
    EmploymentType, Gender, PayPeriod, PayrollMessageCode, SeparationReason, TaxRegime,
};
use crate::india::{epf, esi, gratuity, pt, tds};
use crate::money::{dec, max, min, pct, round2, round_rupee, zero};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayLineInput {
    pub code: String,
    pub label: String,
    pub amount: Decimal,
    #[serde(default)]
    pub taxable: bool,
}

impl PayLineInput {
    pub fn new(code: &str, label: &str, amount: Decimal) -> Self {
        PayLineInput {
            code: code.to_string(),
            label: label.to_string(),
            amount,
            taxable: true,
        }
    }

    pub fn non_taxable(code: &str, label: &str, amount: Decimal) -> Self {
        PayLineInput {
            code: code.to_string(),
            label: label.to_string(),
            amount,
            taxable: false,
        }
    }
}

/// Everything the engine needs for one employee-month.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayrollInput {
    pub employee_id: Uuid,
    pub period: PayPeriod,
    /// Attendance: days paid in the month (paid days + paid leave).
    pub paid_days: Decimal,
    /// Monthly salary components (pre-proration).
    pub basic: Decimal,
    pub dearness_allowance: Decimal,
    pub hra: Decimal,
    pub conveyance: Decimal,
    pub special_allowance: Decimal,
    pub other_allowances: Decimal,
    /// Variable pay: incentive/commission, overtime, bonus, arrears.
    pub incentive: Decimal,
    pub bonus: Decimal,
    pub arrears: Decimal,
    pub overtime_hours: Decimal,
    pub overtime_multiplier: Decimal,
    /// Reimbursements are outside statutory wages and outside TDS.
    pub reimbursements: Decimal,
    #[serde(default)]
    pub other_earnings: Vec<PayLineInput>,
    #[serde(default)]
    pub other_deductions: Vec<PayLineInput>,
    pub pf_applicable: bool,
    pub pf_wage_override: Option<Decimal>,
    pub esi_applicable: bool,
    pub tax_regime: TaxRegime,
    #[serde(default)]
    pub declarations: tds::TaxDeclarations,
    /// Place of work (ISO 3166-2:IN code: MH, KA, WB, TN, ...).
    pub work_state: String,
    pub gender: Gender,
    pub disability: bool,
    pub employment_type: EmploymentType,
    /// Completed years of continuous service (drives gratuity).
    pub service_years: u32,
    #[serde(default)]
    pub service_months_final_year: u32,
    /// Override for the annualised gross used for TDS (e.g. known variable pay).
    pub annual_gross_projection: Option<Decimal>,
    /// TDS already deducted in this financial year.
    pub ytd_tds: Decimal,
    /// Annual gross remuneration (fixed pay + variable pay, excluding the
    /// employer's statutory contributions) used for the 50% wage check.
    pub ctc_annual: Option<Decimal>,
    pub pay_data_changed: bool,
    pub regime_changed: bool,
}

impl PayrollInput {
    pub fn new(employee_id: Uuid, period: PayPeriod) -> Self {
        PayrollInput {
            employee_id,
            period,
            paid_days: Decimal::from(period.days_in_month()),
            basic: zero(),
            dearness_allowance: zero(),
            hra: zero(),
            conveyance: zero(),
            special_allowance: zero(),
            other_allowances: zero(),
            incentive: zero(),
            bonus: zero(),
            arrears: zero(),
            overtime_hours: zero(),
            overtime_multiplier: Decimal::from(2),
            reimbursements: zero(),
            other_earnings: Vec::new(),
            other_deductions: Vec::new(),
            pf_applicable: true,
            pf_wage_override: None,
            esi_applicable: true,
            tax_regime: TaxRegime::New,
            declarations: tds::TaxDeclarations::default(),
            work_state: "KA".into(),
            gender: Gender::Male,
            disability: false,
            employment_type: EmploymentType::FullTime,
            service_years: 0,
            service_months_final_year: 0,
            annual_gross_projection: None,
            ytd_tds: zero(),
            ctc_annual: None,
            pay_data_changed: false,
            regime_changed: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayLine {
    pub code: String,
    pub label: String,
    pub amount: Decimal,
    pub taxable: bool,
    pub statutory: bool,
}

impl PayLine {
    pub fn earning(code: &str, label: &str, amount: Decimal, taxable: bool) -> Self {
        PayLine {
            code: code.into(),
            label: label.into(),
            amount,
            taxable,
            statutory: false,
        }
    }

    pub fn statutory(code: &str, label: &str, amount: Decimal) -> Self {
        PayLine {
            code: code.into(),
            label: label.into(),
            amount,
            taxable: false,
            statutory: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayrollMessage {
    pub code: String,
    pub severity: String,
    pub text: String,
}

impl PayrollMessage {
    fn new(code: PayrollMessageCode, text: impl Into<String>) -> Self {
        PayrollMessage {
            code: code.code().to_string(),
            severity: code.severity().to_string(),
            text: text.into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PayslipComputation {
    pub employee_id: Uuid,
    pub period: PayPeriod,
    pub days_in_month: u32,
    pub paid_days: Decimal,
    pub lop_days: Decimal,
    pub earnings: Vec<PayLine>,
    pub deductions: Vec<PayLine>,
    pub employer_contributions: Vec<PayLine>,
    pub gross_earnings: Decimal,
    pub total_deductions: Decimal,
    pub net_pay: Decimal,
    pub employer_cost: Decimal,
    pub pf: epf::EpfSplit,
    pub esi: esi::EsiContribution,
    pub professional_tax: pt::PtComputation,
    pub tds: tds::TaxBreakdown,
    pub tds_monthly: Decimal,
    pub gratuity_accrual: Decimal,
    pub gratuity_eligible: bool,
    pub messages: Vec<PayrollMessage>,
}

impl PayslipComputation {
    pub fn message_codes(&self) -> Vec<String> {
        self.messages.iter().map(|m| m.code.clone()).collect()
    }

    pub fn has_errors(&self) -> bool {
        self.messages.iter().any(|m| m.severity == "error")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PayrollError {
    #[error("invalid payroll input: {0}")]
    InvalidInput(String),
}

#[derive(Debug, Clone)]
pub struct PayrollEngine {
    config: IndiaPayrollConfig,
}

impl PayrollEngine {
    pub fn new(config: IndiaPayrollConfig) -> Self {
        PayrollEngine { config }
    }

    pub fn config(&self) -> &IndiaPayrollConfig {
        &self.config
    }

    /// Compute a payslip.
    pub fn compute(&self, input: &PayrollInput) -> Result<PayslipComputation, PayrollError> {
        if input.basic < zero() {
            return Err(PayrollError::InvalidInput("basic cannot be negative".into()));
        }
        let days_in_month = input.period.days_in_month();
        let max_days = Decimal::from(days_in_month);
        if input.paid_days < zero() {
            return Err(PayrollError::InvalidInput("paid_days cannot be negative".into()));
        }
        if input.paid_days > max_days {
            return Err(PayrollError::InvalidInput(format!(
                "paid_days {} exceeds {} days in {}",
                input.paid_days,
                days_in_month,
                input.period.label()
            )));
        }

        let paid_days = input.paid_days;
        let lop_days = max_days - paid_days;
        let factor = if paid_days == max_days || max_days.is_zero() {
            Decimal::ONE
        } else {
            paid_days / max_days
        };

        let mut messages = Vec::new();
        if lop_days > zero() && paid_days > zero() {
            messages.push(PayrollMessage::new(
                PayrollMessageCode::PartialPeriod,
                format!(
                    "Paid {paid_days} of {days_in_month} days; {lop_days} LOP day(s) deducted"
                ),
            ));
        }
        if paid_days.is_zero() {
            messages.push(PayrollMessage::new(
                PayrollMessageCode::NoPay,
                "No paid days in the period — a nil payslip is produced for the record",
            ));
        }
        if input.pay_data_changed {
            messages.push(PayrollMessage::new(
                PayrollMessageCode::PayDataChange,
                "Pay data changed after the payroll cut-off (retro adjustment applied)",
            ));
        }
        if input.regime_changed {
            messages.push(PayrollMessage::new(
                PayrollMessageCode::TaxRegimeChange,
                format!("Tax regime elected: {}", input.tax_regime.as_str()),
            ));
        }

        // ---- earnings (prorated by paid days) -----------------------
        let basic = round2(input.basic * factor);
        let da = round2(input.dearness_allowance * factor);
        let hra = round2(input.hra * factor);
        let conveyance = round2(input.conveyance * factor);
        let special = round2(input.special_allowance * factor);
        let other_allow = round2(input.other_allowances * factor);

        let hourly_rate = if days_in_month > 0 {
            input.basic / Decimal::from(days_in_month) / Decimal::from(8)
        } else {
            zero()
        };
        let overtime = round2(hourly_rate * input.overtime_multiplier * input.overtime_hours);
        let incentive = round2(input.incentive);
        let bonus = round2(input.bonus);
        let arrears = round2(input.arrears);
        let reimbursements = round2(input.reimbursements);

        let mut earnings: Vec<PayLine> = Vec::new();
        let mut push = |line: PayLine| {
            if !line.amount.is_zero() {
                earnings.push(line);
            }
        };
        push(PayLine::earning("BASIC", "Basic Pay", basic, true));
        push(PayLine::earning(
            "DA",
            "Dearness Allowance",
            da,
            true,
        ));
        push(PayLine::earning("HRA", "House Rent Allowance", hra, true));
        push(PayLine::earning("CONV", "Conveyance", conveyance, true));
        push(PayLine::earning(
            "SPECIAL",
            "Special Allowance",
            special,
            true,
        ));
        push(PayLine::earning(
            "OTHER_ALLOW",
            "Other Allowances",
            other_allow,
            true,
        ));
        push(PayLine::earning(
            "OT",
            "Overtime",
            overtime,
            true,
        ));
        push(PayLine::earning(
            "INCENTIVE",
            "Incentive / Commission",
            incentive,
            true,
        ));
        push(PayLine::earning("BONUS", "Bonus", bonus, true));
        push(PayLine::earning("ARREARS", "Arrears", arrears, true));
        for extra in &input.other_earnings {
            push(PayLine::earning(
                &extra.code,
                &extra.label,
                round2(extra.amount),
                extra.taxable,
            ));
        }
        if !reimbursements.is_zero() {
            earnings.push(PayLine::earning(
                "REIMB",
                "Reimbursements (bill-based)",
                reimbursements,
                false,
            ));
        }

        let gross_earnings: Decimal = round2(earnings.iter().map(|l| l.amount).sum());
        if !overtime.is_zero() {
            messages.push(PayrollMessage::new(
                PayrollMessageCode::OvertimePaid,
                format!("{:.2} OT hour(s) at {:.2}x", input.overtime_hours, input.overtime_multiplier),
            ));
        }
        if !bonus.is_zero() || !incentive.is_zero() {
            messages.push(PayrollMessage::new(
                PayrollMessageCode::BonusPaid,
                format!("Variable pay this month: incentives {incentive}, bonus {bonus}"),
            ));
        }

        let taxable_earnings: Decimal = round2(
            earnings
                .iter()
                .filter(|l| l.taxable)
                .map(|l| l.amount)
                .sum(),
        );

        // ---- EPF ----------------------------------------------------
        let pf = epf::compute(
            &self.config.epf,
            basic + da,
            input.period.first_day(),
            input.pf_wage_override,
            input.pf_applicable,
        );
        if pf.ceiling_applied {
            messages.push(PayrollMessage::new(
                PayrollMessageCode::PfCeilingApplied,
                format!(
                    "PF wages capped at the statutory ceiling of Rs.{} (basic+DA Rs.{})",
                    pf.ceiling, basic + da
                ),
            ));
        }

        // ---- ESI ----------------------------------------------------
        let esi_wages = max(gross_earnings - reimbursements, zero());
        let esi = esi::compute(&self.config.esi, esi_wages, input.disability, input.esi_applicable);
        if input.esi_applicable && !esi.applicable && esi_wages > esi.ceiling {
            messages.push(PayrollMessage::new(
                PayrollMessageCode::EsiCeilingExceeded,
                format!(
                    "Gross Rs.{esi_wages} is above the ESI ceiling of Rs.{} — ESI not deducted",
                    esi.ceiling
                ),
            ));
        }

        // ---- Professional tax (place of work) -----------------------
        let professional_tax = pt::compute(
            &self.config.professional_tax,
            &input.work_state,
            esi_wages,
            input.period,
            input.gender,
        );
        if !professional_tax.applicable {
            messages.push(PayrollMessage::new(
                PayrollMessageCode::PtStateNotConfigured,
                format!(
                    "Professional tax not applied: work state '{}' has no configured slab",
                    input.work_state
                ),
            ));
        }

        // ---- TDS ----------------------------------------------------
        let projected_annual_gross = input
            .annual_gross_projection
            .unwrap_or_else(|| round2(taxable_earnings * Decimal::from(12)));
        let tax = tds::annual_tax(
            &self.config.tds,
            input.tax_regime,
            projected_annual_gross,
            &input.declarations,
        );
        let months_remaining = input.period.months_remaining_in_fy();
        let tds_monthly = tds::monthly_tds(tax.total_tax, input.ytd_tds, months_remaining);

        // ---- deductions --------------------------------------------
        let mut deductions: Vec<PayLine> = Vec::new();
        if pf.employee > zero() {
            deductions.push(PayLine::statutory("PF_EE", "EPF — Employee (12%)", pf.employee));
        }
        if esi.employee > zero() {
            deductions.push(PayLine::statutory("ESI_EE", "ESI — Employee (0.75%)", esi.employee));
        }
        if professional_tax.amount > zero() {
            deductions.push(PayLine::statutory(
                "PT",
                &format!("Professional Tax ({})", professional_tax.state),
                professional_tax.amount,
            ));
        }
        if tds_monthly > zero() {
            deductions.push(PayLine::statutory("TDS", "TDS — Income Tax", tds_monthly));
        }
        for extra in &input.other_deductions {
            if !extra.amount.is_zero() {
                deductions.push(PayLine::statutory(
                    &extra.code,
                    &extra.label,
                    round2(extra.amount),
                ));
            }
        }
        let total_deductions: Decimal = round2(deductions.iter().map(|l| l.amount).sum());
        let net_pay = round2(gross_earnings - total_deductions);
        if net_pay < zero() {
            messages.push(PayrollMessage::new(
                PayrollMessageCode::NegativeNet,
                "Deductions exceed earnings for this period",
            ));
        }

        // ---- employer contributions + gratuity provision ------------
        let gratuity_monthly = gratuity::monthly_accrual(&self.config.gratuity, basic + da);
        let gratuity_eligible = gratuity::settlement(
            &self.config.gratuity,
            basic + da,
            input.service_years,
            input.service_months_final_year,
            input.employment_type,
            SeparationReason::Resignation,
        )
        .eligible;

        let mut employer_contributions: Vec<PayLine> = Vec::new();
        if pf.employer_epf > zero() {
            employer_contributions.push(PayLine::statutory(
                "PF_ER",
                "EPF — Employer (3.67%)",
                pf.employer_epf,
            ));
        }
        if pf.employer_eps > zero() {
            employer_contributions.push(PayLine::statutory(
                "EPS_ER",
                "EPS — Employer (8.33%)",
                pf.employer_eps,
            ));
        }
        if pf.edli > zero() {
            employer_contributions.push(PayLine::statutory("EDLI", "EDLI (0.5%)", pf.edli));
        }
        if pf.admin_charges > zero() {
            employer_contributions.push(PayLine::statutory(
                "PF_ADMIN",
                "EPF Admin Charges (0.5%)",
                pf.admin_charges,
            ));
        }
        if esi.employer > zero() {
            employer_contributions.push(PayLine::statutory(
                "ESI_ER",
                "ESI — Employer (3.25%)",
                esi.employer,
            ));
        }
        if gratuity_monthly > zero() {
            employer_contributions.push(PayLine::statutory(
                "GRATUITY",
                "Gratuity Provision",
                gratuity_monthly,
            ));
        }
        if !gratuity_eligible && input.service_years < self.config.gratuity.min_service_years {
            messages.push(PayrollMessage::new(
                PayrollMessageCode::GratuityNotEligible,
                format!(
                    "Gratuity provision booked but not yet payable ({} of {} years)",
                    input.service_years, self.config.gratuity.min_service_years
                ),
            ));
        }

        let employer_cost = round2(
            gross_earnings
                + pf.employer_total
                + pf.edli
                + pf.admin_charges
                + esi.employer
                + gratuity_monthly,
        );

        // ---- Code on Wages: core wages >= 50% of remuneration -------
        if self.config.gratuity.wage_50pct_rule && gross_earnings > zero() {
            let core = basic + da;
            let half = pct(gross_earnings, dec(0.5));
            if core < half {
                messages.push(PayrollMessage::new(
                    PayrollMessageCode::Wage50PctRule,
                    format!(
                        "Basic+DA (Rs.{core}) is below 50% of gross (Rs.{gross_earnings}) — Code on Wages s.2(y) breach; restructure the salary"
                    ),
                ));
            }
        }
        if let Some(ctc) = input.ctc_annual {
            if ctc > zero() {
                let core_annual = (basic + da) * Decimal::from(12);
                if core_annual < pct(ctc, dec(0.5)) {
                    messages.push(PayrollMessage::new(
                        PayrollMessageCode::Wage50PctRule,
                        format!(
                            "Annual basic+DA Rs.{core_annual} is below 50% of CTC Rs.{ctc}"
                        ),
                    ));
                }
            }
        }

        Ok(PayslipComputation {
            employee_id: input.employee_id,
            period: input.period,
            days_in_month,
            paid_days,
            lop_days,
            earnings,
            deductions,
            employer_contributions,
            gross_earnings,
            total_deductions,
            net_pay,
            employer_cost,
            pf,
            esi,
            professional_tax,
            tds: tax,
            tds_monthly,
            gratuity_accrual: gratuity_monthly,
            gratuity_eligible,
            messages,
        })
    }

    /// Build the reversing entry for an already-processed payslip (PeopleSoft
    /// "Payroll Reversal"). Amounts are negated and the REVERSAL message is
    /// attached; statutory detail is retained for the audit trail.
    pub fn reversal(&self, input: &PayrollInput) -> Result<PayslipComputation, PayrollError> {
        let mut slip = self.compute(input)?;
        for line in slip
            .earnings
            .iter_mut()
            .chain(slip.deductions.iter_mut())
            .chain(slip.employer_contributions.iter_mut())
        {
            line.amount = -line.amount;
        }
        slip.gross_earnings = -slip.gross_earnings;
        slip.total_deductions = -slip.total_deductions;
        slip.net_pay = -slip.net_pay;
        slip.employer_cost = -slip.employer_cost;
        slip.tds_monthly = -slip.tds_monthly;
        slip.gratuity_accrual = -slip.gratuity_accrual;
        slip.pf.employee = -slip.pf.employee;
        slip.pf.employer_total = -slip.pf.employer_total;
        slip.pf.employer_epf = -slip.pf.employer_epf;
        slip.pf.employer_eps = -slip.pf.employer_eps;
        slip.pf.edli = -slip.pf.edli;
        slip.pf.admin_charges = -slip.pf.admin_charges;
        slip.esi.employee = -slip.esi.employee;
        slip.esi.employer = -slip.esi.employer;
        slip.professional_tax.amount = -slip.professional_tax.amount;
        slip.messages.push(PayrollMessage::new(
            PayrollMessageCode::Reversal,
            "Reversal of a previously processed payslip; a corrected run must be posted",
        ));
        Ok(slip)
    }

    /// Convenience wrapper used by the compliance endpoints.
    pub fn professional_tax_liability(
        &self,
        state: &str,
        gross: Decimal,
        period: PayPeriod,
        gender: Gender,
    ) -> pt::PtComputation {
        pt::compute(&self.config.professional_tax, state, gross, period, gender)
    }

    pub fn gratuity_settlement(
        &self,
        wages: Decimal,
        years: u32,
        months: u32,
        employment_type: EmploymentType,
        reason: SeparationReason,
    ) -> gratuity::GratuityResult {
        gratuity::settlement(
            &self.config.gratuity,
            wages,
            years,
            months,
            employment_type,
            reason,
        )
    }

    pub fn annual_tax_projection(
        &self,
        regime: TaxRegime,
        gross_annual: Decimal,
        declarations: &tds::TaxDeclarations,
    ) -> tds::TaxBreakdown {
        tds::annual_tax(&self.config.tds, regime, gross_annual, declarations)
    }

    pub fn unpaid_days_ratio(paid_days: Decimal, days_in_month: u32) -> Decimal {
        if days_in_month == 0 {
            return Decimal::ONE;
        }
        min(paid_days, Decimal::from(days_in_month)) / Decimal::from(days_in_month)
    }

    /// Rounding helper shared by the compliance exporters.
    pub fn round_rupee(value: Decimal) -> Decimal {
        round_rupee(value)
    }
}
