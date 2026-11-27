//! Shared domain vocabulary: roles, tax regimes, pay periods, payroll message
//! codes (the PeopleSoft "payroll message" concept) and call-centre aux states.

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

/// RBAC roles. `SuperAdmin` is all-powerful; the remaining roles mirror the
/// PeopleSoft HCM permission lists used by a BPO HR shared-service centre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    SuperAdmin,
    HrAdmin,
    PayrollAdmin,
    BenefitsAdmin,
    Recruiter,
    Manager,
    TeamLead,
    Agent,
    QaAnalyst,
    Trainer,
}

impl Role {
    pub const ALL: [Role; 10] = [
        Role::SuperAdmin,
        Role::HrAdmin,
        Role::PayrollAdmin,
        Role::BenefitsAdmin,
        Role::Recruiter,
        Role::Manager,
        Role::TeamLead,
        Role::Agent,
        Role::QaAnalyst,
        Role::Trainer,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Role::SuperAdmin => "super_admin",
            Role::HrAdmin => "hr_admin",
            Role::PayrollAdmin => "payroll_admin",
            Role::BenefitsAdmin => "benefits_admin",
            Role::Recruiter => "recruiter",
            Role::Manager => "manager",
            Role::TeamLead => "team_lead",
            Role::Agent => "agent",
            Role::QaAnalyst => "qa_analyst",
            Role::Trainer => "trainer",
        }
    }

    pub fn from_str_opt(value: &str) -> Option<Role> {
        Role::ALL.iter().copied().find(|r| r.as_str() == value)
    }

    /// Roles that maintain person/job data.
    pub fn is_people_admin(&self) -> bool {
        matches!(self, Role::SuperAdmin | Role::HrAdmin)
    }

    pub fn can_manage_compensation(&self) -> bool {
        matches!(self, Role::SuperAdmin | Role::HrAdmin | Role::PayrollAdmin)
    }

    pub fn can_run_payroll(&self) -> bool {
        matches!(self, Role::SuperAdmin | Role::PayrollAdmin)
    }

    pub fn can_manage_benefits(&self) -> bool {
        matches!(self, Role::SuperAdmin | Role::HrAdmin | Role::BenefitsAdmin)
    }

    pub fn can_recruit(&self) -> bool {
        matches!(self, Role::SuperAdmin | Role::HrAdmin | Role::Recruiter)
    }

    pub fn can_approve_time(&self) -> bool {
        matches!(
            self,
            Role::SuperAdmin | Role::HrAdmin | Role::Manager | Role::TeamLead
        )
    }

    pub fn can_deliver_training(&self) -> bool {
        matches!(self, Role::SuperAdmin | Role::HrAdmin | Role::Trainer)
    }

}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaxRegime {
    New,
    Old,
}

impl TaxRegime {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaxRegime::New => "new",
            TaxRegime::Old => "old",
        }
    }

    pub fn from_str_opt(value: &str) -> Option<TaxRegime> {
        match value {
            "new" => Some(TaxRegime::New),
            "old" => Some(TaxRegime::Old),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Gender {
    Male,
    Female,
    Other,
}

impl Gender {
    pub fn is_female(&self) -> bool {
        matches!(self, Gender::Female)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmploymentType {
    FullTime,
    PartTime,
    FixedTerm,
    Contractor,
    Intern,
}

impl EmploymentType {
    pub fn from_str_opt(value: &str) -> Option<Self> {
        Some(
            match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
                "full_time" | "fulltime" | "permanent" => EmploymentType::FullTime,
                "part_time" | "parttime" => EmploymentType::PartTime,
                "fixed_term" | "fixedterm" | "contract" => EmploymentType::FixedTerm,
                "contractor" | "consultant" => EmploymentType::Contractor,
                "intern" | "internship" | "trainee" => EmploymentType::Intern,
                _ => return None,
            },
        )
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            EmploymentType::FullTime => "full_time",
            EmploymentType::PartTime => "part_time",
            EmploymentType::FixedTerm => "fixed_term",
            EmploymentType::Contractor => "contractor",
            EmploymentType::Intern => "intern",
        }
    }
}

/// Termination reason drives the gratuity eligibility waiver
/// (death / disablement waive the 5-year rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeparationReason {
    Resignation,
    Retirement,
    Termination,
    Death,
    Disablement,
    FixedTermCompletion,
}

/// A payroll period (calendar month).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayPeriod {
    pub year: i32,
    pub month: u32,
}

impl PayPeriod {
    pub fn new(year: i32, month: u32) -> Self {
        let month = month.clamp(1, 12);
        PayPeriod { year, month }
    }

    pub fn from_date(date: NaiveDate) -> Self {
        PayPeriod {
            year: date.year(),
            month: date.month(),
        }
    }

    pub fn days_in_month(&self) -> u32 {
        let first = NaiveDate::from_ymd_opt(self.year, self.month, 1).expect("valid month");
        let next = if self.month == 12 {
            NaiveDate::from_ymd_opt(self.year + 1, 1, 1)
        } else {
            NaiveDate::from_ymd_opt(self.year, self.month + 1, 1)
        }
        .expect("valid month");
        (next - first).num_days() as u32
    }

    pub fn first_day(&self) -> NaiveDate {
        NaiveDate::from_ymd_opt(self.year, self.month, 1).expect("valid month")
    }

    pub fn last_day(&self) -> NaiveDate {
        let first = self.first_day();
        let next = if self.month == 12 {
            NaiveDate::from_ymd_opt(self.year + 1, 1, 1)
        } else {
            NaiveDate::from_ymd_opt(self.year, self.month + 1, 1)
        }
        .expect("valid month");
        next.pred_opt().unwrap_or(first)
    }

    pub fn label(&self) -> String {
        format!("{:04}-{:02}", self.year, self.month)
    }

    /// Indian financial year starts in April.
    pub fn fy_start_year(&self) -> i32 {
        if self.month >= 4 {
            self.year
        } else {
            self.year - 1
        }
    }

    pub fn fy_label(&self) -> String {
        let start = self.fy_start_year();
        let end_short = (start + 1) % 100;
        format!("FY {}-{:02}", start, end_short)
    }

    /// Months left in the financial year including this one (used to spread
    /// the balance of annual TDS).
    pub fn months_remaining_in_fy(&self) -> u8 {
        let month_index = if self.month >= 4 {
            self.month - 4
        } else {
            self.month + 8
        };
        (12 - month_index) as u8
    }

    pub fn previous(&self) -> PayPeriod {
        if self.month == 1 {
            PayPeriod::new(self.year - 1, 12)
        } else {
            PayPeriod::new(self.year, self.month - 1)
        }
    }

    pub fn next(&self) -> PayPeriod {
        if self.month == 12 {
            PayPeriod::new(self.year + 1, 1)
        } else {
            PayPeriod::new(self.year, self.month + 1)
        }
    }
}

/// PeopleSoft-style payroll message codes. Every payslip carries the codes
/// that explain how it was produced, so payroll can be audited without
/// re-running the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PayrollMessageCode {
    PartialPeriod,
    PayDataChange,
    Reversal,
    Adjustment,
    NoPay,
    TaxRegimeChange,
    PfCeilingApplied,
    EsiCeilingExceeded,
    PtStateNotConfigured,
    Wage50PctRule,
    NegativeNet,
    GratuityNotEligible,
    OvertimePaid,
    BonusPaid,
}

impl PayrollMessageCode {
    pub fn code(&self) -> &'static str {
        match self {
            PayrollMessageCode::PartialPeriod => "PARTIAL_PERIOD",
            PayrollMessageCode::PayDataChange => "PAY_DATA_CHANGE",
            PayrollMessageCode::Reversal => "REVERSAL",
            PayrollMessageCode::Adjustment => "ADJUSTMENT",
            PayrollMessageCode::NoPay => "NO_PAY",
            PayrollMessageCode::TaxRegimeChange => "TAX_REGIME_CHANGE",
            PayrollMessageCode::PfCeilingApplied => "PF_CEILING_APPLIED",
            PayrollMessageCode::EsiCeilingExceeded => "ESI_CEILING_EXCEEDED",
            PayrollMessageCode::PtStateNotConfigured => "PT_STATE_NOT_CONFIGURED",
            PayrollMessageCode::Wage50PctRule => "WAGE_50PCT_RULE",
            PayrollMessageCode::NegativeNet => "NEGATIVE_NET",
            PayrollMessageCode::GratuityNotEligible => "GRATUITY_NOT_ELIGIBLE",
            PayrollMessageCode::OvertimePaid => "OVERTIME_PAID",
            PayrollMessageCode::BonusPaid => "BONUS_PAID",
        }
    }

    pub fn severity(&self) -> &'static str {
        match self {
            PayrollMessageCode::NegativeNet
            | PayrollMessageCode::Reversal
            | PayrollMessageCode::Wage50PctRule
            | PayrollMessageCode::PtStateNotConfigured => "error",
            PayrollMessageCode::PartialPeriod
            | PayrollMessageCode::NoPay
            | PayrollMessageCode::EsiCeilingExceeded
            | PayrollMessageCode::PfCeilingApplied
            | PayrollMessageCode::GratuityNotEligible => "warning",
            _ => "info",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn financial_year_maths() {
        let p = PayPeriod::new(2026, 4);
        assert_eq!(p.fy_label(), "FY 2026-27");
        assert_eq!(p.months_remaining_in_fy(), 12);
        let p = PayPeriod::new(2026, 11);
        assert_eq!(p.fy_label(), "FY 2026-27");
        assert_eq!(p.months_remaining_in_fy(), 5);
        let p = PayPeriod::new(2027, 3);
        assert_eq!(p.fy_label(), "FY 2026-27");
        assert_eq!(p.months_remaining_in_fy(), 1);
        assert_eq!(p.next().label(), "2027-04");
    }

    #[test]
    fn days_in_month_handles_leap_years() {
        assert_eq!(PayPeriod::new(2024, 2).days_in_month(), 29);
        assert_eq!(PayPeriod::new(2025, 2).days_in_month(), 28);
        assert_eq!(PayPeriod::new(2026, 11).days_in_month(), 30);
    }

    #[test]
    fn role_round_trip() {
        for role in Role::ALL {
            assert_eq!(Role::from_str_opt(role.as_str()), Some(role));
        }
    }
}
