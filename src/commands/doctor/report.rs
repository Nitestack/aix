use super::failure::CheckFailure;
use crate::error::DoctorFailureCategory;
use serde::Serialize;
use std::io::{self, Write};

pub(super) struct DoctorReport {
    pub(super) checks: Vec<DoctorCheck>,
    pub(super) failure_category: Option<DoctorFailureCategory>,
}

impl DoctorReport {
    pub(super) fn new() -> Self {
        Self {
            checks: Vec::with_capacity(9),
            failure_category: None,
        }
    }

    pub(super) fn pass(
        &mut self,
        name: &'static str,
        message: impl Into<String>,
        duration_ms: Option<u64>,
    ) {
        self.checks.push(DoctorCheck::new(
            name,
            CheckStatus::Pass,
            message,
            duration_ms,
        ));
    }

    pub(super) fn fail(
        &mut self,
        name: &'static str,
        message: impl Into<String>,
        hint: &'static str,
        duration_ms: Option<u64>,
        category: DoctorFailureCategory,
    ) {
        self.fail_with(
            name,
            CheckFailure::new(message, hint, category),
            duration_ms,
        );
    }

    pub(super) fn fail_with(
        &mut self,
        name: &'static str,
        failure: CheckFailure,
        duration_ms: Option<u64>,
    ) {
        if self.failure_category.is_none() {
            self.failure_category = Some(failure.category);
        }
        self.checks.push(DoctorCheck::new(
            name,
            CheckStatus::Fail,
            format!("{}; hint: {}", failure.message, failure.hint),
            duration_ms,
        ));
    }

    pub(super) fn skipped(&mut self, name: &'static str, reason: &str) {
        self.checks.push(DoctorCheck::new(
            name,
            CheckStatus::Skipped,
            format!("skipped: {reason}"),
            None,
        ));
    }

    pub(super) fn not_applicable(&mut self, name: &'static str, reason: &str) {
        self.checks.push(DoctorCheck::new(
            name,
            CheckStatus::NotApplicable,
            format!("not applicable: {reason}"),
            None,
        ));
    }
}

#[derive(Serialize)]
pub(super) struct DoctorOutput<'a> {
    pub(super) checks: &'a [DoctorCheck],
}

#[derive(Serialize)]
pub(super) struct DoctorCheck {
    name: &'static str,
    status: CheckStatus,
    message: String,
    duration_ms: Option<u64>,
}

impl DoctorCheck {
    fn new(
        name: &'static str,
        status: CheckStatus,
        message: impl Into<String>,
        duration_ms: Option<u64>,
    ) -> Self {
        Self {
            name,
            status,
            message: message.into(),
            duration_ms,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum CheckStatus {
    Pass,
    Fail,
    Skipped,
    NotApplicable,
}

pub(super) fn print_human(checks: &[DoctorCheck], output: &mut impl Write) -> io::Result<()> {
    for check in checks {
        let status = match check.status {
            CheckStatus::Pass => "PASS",
            CheckStatus::Fail => "FAIL",
            CheckStatus::Skipped => "SKIP",
            CheckStatus::NotApplicable => "N/A",
        };
        writeln!(output, "{status:4} {} — {}", check.name, check.message)?;
    }
    Ok(())
}
