//! `hrms-core` — domain vocabulary, the India statutory payroll engine, and
//! compliance export builders.
//!
//! The crate is deliberately free of I/O (except reading the config file) so
//! the statutory rules can be unit-tested without a database, HTTP server or
//! Docker.

pub mod compliance;
pub mod config;
pub mod domain;
pub mod india;
pub mod money;

pub use config::{ConfigError, IndiaPayrollConfig};
pub use domain::{
    AuxCategory, DashboardScope, EmploymentType, Gender, PayPeriod, PayrollMessageCode, Role,
    SeparationReason, TaxRegime,
};
pub use india::engine::{
    PayLine, PayLineInput, PayrollEngine, PayrollError, PayrollInput, PayrollMessage,
    PayslipComputation,
};
pub use india::epf::EpfSplit;
pub use india::esi::EsiContribution;
pub use india::gratuity::GratuityResult;
pub use india::pt::PtComputation;
pub use india::tds::{TaxBreakdown, TaxDeclarations};
