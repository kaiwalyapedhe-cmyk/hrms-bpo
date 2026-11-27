//! Typed view of `config/india_payroll.toml`. The file shipped with the repo is
//! also embedded as the compile-time default, so the API still boots with sane
//! statutory behaviour if the mounted config is missing (and a warning is
//! logged).

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::money::{dec, min};

pub const EMBEDDED_CONFIG: &str = include_str!("../../../config/india_payroll.toml");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndiaPayrollConfig {
    #[serde(default)]
    pub meta: MetaConfig,
    pub epf: EpfConfig,
    pub esi: EsiConfig,
    pub tds: TdsConfig,
    pub professional_tax: ProfessionalTaxConfig,
    pub gratuity: GratuityConfig,
    #[serde(default)]
    pub compliance: ComplianceConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaConfig {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub assessment_year: String,
    #[serde(default)]
    pub financial_year: String,
    #[serde(default)]
    pub income_tax_slab_year: String,
    #[serde(default = "default_currency")]
    pub currency: String,
    #[serde(default)]
    pub reviewed_on: String,
}

fn default_currency() -> String {
    "INR".to_string()
}

impl Default for MetaConfig {
    fn default() -> Self {
        MetaConfig {
            version: "2026.11".into(),
            assessment_year: "2026-27".into(),
            financial_year: "2026-27".into(),
            income_tax_slab_year: "FY 2025-26".into(),
            currency: default_currency(),
            reviewed_on: "2026-11-26".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpfConfig {
    pub employee_rate: f64,
    pub employer_rate: f64,
    pub eps_rate: f64,
    pub wage_ceiling: f64,
    pub eps_wage_ceiling: f64,
    #[serde(default)]
    pub admin_charge_rate: f64,
    #[serde(default)]
    pub edli_rate: f64,
    #[serde(default = "default_true")]
    pub apply_ceiling_to_employee: bool,
    #[serde(default)]
    pub ceiling_hike: CeilingHike,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CeilingHike {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_hike_ceiling")]
    pub wage_ceiling: f64,
    #[serde(default = "default_hike_ceiling")]
    pub eps_wage_ceiling: f64,
    #[serde(default = "default_hike_date")]
    pub effective_from: String,
    #[serde(default)]
    pub note: String,
}

fn default_hike_ceiling() -> f64 {
    25_000.0
}

fn default_hike_date() -> String {
    "2026-09-17".to_string()
}

impl Default for CeilingHike {
    fn default() -> Self {
        CeilingHike {
            enabled: false,
            wage_ceiling: default_hike_ceiling(),
            eps_wage_ceiling: default_hike_ceiling(),
            effective_from: default_hike_date(),
            note: "Announced hike to Rs.25,000 effective 17-Sep-2026.".into(),
        }
    }
}

impl EpfConfig {
    fn hike_from(&self) -> Option<NaiveDate> {
        NaiveDate::parse_from_str(&self.ceiling_hike.effective_from, "%Y-%m-%d").ok()
    }

    /// Statutory PF wage ceiling applicable to a wage month.
    pub fn wage_ceiling_on(&self, on: NaiveDate) -> Decimal {
        if let (true, Some(from)) = (self.ceiling_hike.enabled, self.hike_from()) {
            if on >= from {
                return dec(self.ceiling_hike.wage_ceiling);
            }
        }
        dec(self.wage_ceiling)
    }

    /// EPS diversion ceiling (capped separately from the EPF ceiling).
    pub fn eps_ceiling_on(&self, on: NaiveDate) -> Decimal {
        if let (true, Some(from)) = (self.ceiling_hike.enabled, self.hike_from()) {
            if on >= from {
                return dec(self.ceiling_hike.eps_wage_ceiling);
            }
        }
        dec(self.eps_wage_ceiling)
    }

    pub fn employee_rate_dec(&self) -> Decimal {
        dec(self.employee_rate)
    }

    pub fn employer_rate_dec(&self) -> Decimal {
        dec(self.employer_rate)
    }

    pub fn eps_rate_dec(&self) -> Decimal {
        dec(self.eps_rate)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EsiConfig {
    pub employee_rate: f64,
    pub employer_rate: f64,
    pub wage_ceiling: f64,
    #[serde(default = "default_disability_ceiling")]
    pub wage_ceiling_disability: f64,
    #[serde(default = "default_true")]
    pub round_up_to_rupee: bool,
}

fn default_disability_ceiling() -> f64 {
    25_000.0
}

impl EsiConfig {
    pub fn ceiling_for(&self, disability: bool) -> Decimal {
        if disability {
            dec(self.wage_ceiling_disability)
        } else {
            dec(self.wage_ceiling)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaxSlab {
    /// `None` on the last slab means "and above".
    #[serde(default)]
    pub up_to: Option<f64>,
    pub rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TdsConfig {
    pub default_regime: String,
    pub cess_rate: f64,
    #[serde(default)]
    pub marginal_relief: bool,
    pub standard_deduction_new: f64,
    pub standard_deduction_old: f64,
    pub rebate_new_income_limit: f64,
    pub rebate_new_max: f64,
    pub rebate_old_income_limit: f64,
    pub rebate_old_max: f64,
    pub cap_80c: f64,
    pub cap_80d: f64,
    pub cap_80ccd_1b: f64,
    pub cap_home_loan_interest: f64,
    #[serde(default = "default_true")]
    pub hra_exemption_enabled: bool,
    pub new_regime_slabs: Vec<TaxSlab>,
    pub old_regime_slabs: Vec<TaxSlab>,
    #[serde(default)]
    pub surcharge_slabs: Vec<TaxSlab>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PtSlab {
    #[serde(default)]
    pub up_to: Option<f64>,
    pub amount: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PtCycle {
    Monthly,
    HalfYearly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PtState {
    pub code: String,
    #[serde(default)]
    pub name: String,
    pub cycle: PtCycle,
    #[serde(default)]
    pub female_exemption_upto: f64,
    #[serde(default)]
    pub february_surcharge: f64,
    pub slabs: Vec<PtSlab>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfessionalTaxConfig {
    pub annual_cap: f64,
    #[serde(default)]
    pub states: Vec<PtState>,
}

impl ProfessionalTaxConfig {
    pub fn state(&self, code: &str) -> Option<&PtState> {
        self.states
            .iter()
            .find(|s| s.code.eq_ignore_ascii_case(code.trim()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GratuityConfig {
    pub min_service_years: u32,
    pub fixed_term_min_years: u32,
    pub wage_cap: f64,
    pub days_numerator: f64,
    pub divisor_days: f64,
    #[serde(default = "default_true")]
    pub wage_50pct_rule: bool,
    #[serde(default = "default_six")]
    pub round_partial_year_months: u32,
}

fn default_six() -> u32 {
    6
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceConfig {
    pub records_retention_years: u32,
    pub pf_filing_day: u32,
    pub esi_filing_day: u32,
    #[serde(default = "default_pt_day")]
    pub pt_filing_day: u32,
    #[serde(default)]
    pub tds_quarterly: bool,
    #[serde(default = "default_fy_month")]
    pub financial_year_start_month: u32,
}

fn default_pt_day() -> u32 {
    20
}

fn default_fy_month() -> u32 {
    4
}

impl Default for ComplianceConfig {
    fn default() -> Self {
        ComplianceConfig {
            records_retention_years: 7,
            pf_filing_day: 15,
            esi_filing_day: 15,
            pt_filing_day: 20,
            tds_quarterly: true,
            financial_year_start_month: 4,
        }
    }
}

impl Default for IndiaPayrollConfig {
    fn default() -> Self {
        toml::from_str(EMBEDDED_CONFIG).expect("embedded config/india_payroll.toml must parse")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read payroll configuration {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot parse payroll configuration {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("invalid payroll configuration: {0}")]
    Invalid(String),
}

impl IndiaPayrollConfig {
    /// Load from an explicit path (used by the seed binary and tests).
    pub fn load_from_path(path: &str) -> Result<Self, ConfigError> {
        let raw = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_string(),
            source,
        })?;
        let cfg: IndiaPayrollConfig = toml::from_str(&raw).map_err(|source| ConfigError::Parse {
            path: path.to_string(),
            source,
        })?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Load from `HRMS_PAYROLL_CONFIG` (default `config/india_payroll.toml`),
    /// falling back to the embedded copy so the service always starts.
    pub fn load() -> Self {
        let path = std::env::var("HRMS_PAYROLL_CONFIG")
            .unwrap_or_else(|_| "config/india_payroll.toml".to_string());
        match Self::load_from_path(&path) {
            Ok(cfg) => cfg,
            Err(err) => {
                eprintln!("warning: {err}; using embedded statutory configuration");
                IndiaPayrollConfig::default()
            }
        }
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.epf.employee_rate <= 0.0 || self.epf.employee_rate > 0.5 {
            return Err(ConfigError::Invalid(
                "epf.employee_rate must be in (0, 0.5]".into(),
            ));
        }
        let split = self.epf.eps_rate + (self.epf.employer_rate - self.epf.eps_rate);
        if (split - self.epf.employer_rate).abs() > 1e-9 {
            return Err(ConfigError::Invalid(
                "epf.eps_rate must not exceed epf.employer_rate".into(),
            ));
        }
        if self.epf.eps_rate > self.epf.employer_rate {
            return Err(ConfigError::Invalid(
                "epf.eps_rate cannot exceed epf.employer_rate".into(),
            ));
        }
        if self.esi.employee_rate + self.esi.employer_rate > 1.0 {
            return Err(ConfigError::Invalid("esi rates must be fractions".into()));
        }
        if self.esi.wage_ceiling_disability < self.esi.wage_ceiling {
            return Err(ConfigError::Invalid(
                "esi.wage_ceiling_disability must be >= esi.wage_ceiling".into(),
            ));
        }
        if self.tds.new_regime_slabs.is_empty() || self.tds.old_regime_slabs.is_empty() {
            return Err(ConfigError::Invalid("tds slab tables cannot be empty".into()));
        }
        if self.professional_tax.states.is_empty() {
            return Err(ConfigError::Invalid(
                "professional_tax.states must contain at least one state".into(),
            ));
        }
        if self.professional_tax.annual_cap > 2500.0 {
            return Err(ConfigError::Invalid(
                "professional_tax.annual_cap exceeds the Article 276 ceiling of Rs.2500".into(),
            ));
        }
        for state in &self.professional_tax.states {
            if state.slabs.is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "professional tax slabs missing for state {}",
                    state.code
                )));
            }
        }
        if self.gratuity.wage_cap <= 0.0 {
            return Err(ConfigError::Invalid("gratuity.wage_cap must be > 0".into()));
        }
        if self.compliance.records_retention_years < 7 {
            return Err(ConfigError::Invalid(
                "compliance.records_retention_years must be at least 7 years".into(),
            ));
        }
        Ok(())
    }

    /// Ceiling that applies to EPS diversion on a given date, never above the
    /// PF wage itself.
    pub fn eps_ceiling_for(&self, on: NaiveDate, pf_wages: Decimal) -> Decimal {
        min(self.epf.eps_ceiling_on(on), pf_wages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_config_is_valid() {
        let cfg = IndiaPayrollConfig::default();
        cfg.validate().expect("embedded config valid");
        assert_eq!(cfg.epf.employee_rate, 0.12);
        assert_eq!(cfg.professional_tax.states.len(), 4);
    }

    #[test]
    fn ceiling_hike_toggle_is_honoured() {
        let mut cfg = IndiaPayrollConfig::default();
        let on = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        assert_eq!(cfg.epf.wage_ceiling_on(on), Decimal::from(15000));
        cfg.epf.ceiling_hike.enabled = true;
        assert_eq!(cfg.epf.wage_ceiling_on(on), Decimal::from(25000));
        // Before the effective date the old ceiling still applies.
        let before = NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
        assert_eq!(cfg.epf.wage_ceiling_on(before), Decimal::from(15000));
    }
}
