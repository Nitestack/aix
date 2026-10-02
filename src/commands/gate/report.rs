use crate::error::GateFailureCategory;
use crate::output;
use color_eyre::Result;
use serde::Serialize;
use serde_json::Value;
use std::io::{self, Write};

pub(super) struct GateReport {
    pub(super) checks: Vec<GateCheck>,
    pub(super) failure_category: Option<GateFailureCategory>,
}

impl GateReport {
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
        details: Option<Value>,
    ) {
        self.checks
            .push(GateCheck::new(name, CheckStatus::Pass, message, details));
    }

    pub(super) fn fail(
        &mut self,
        name: &'static str,
        message: impl Into<String>,
        details: Option<Value>,
        category: GateFailureCategory,
    ) {
        if self.failure_category.is_none() {
            self.failure_category = Some(category);
        }
        self.checks
            .push(GateCheck::new(name, CheckStatus::Fail, message, details));
    }

    pub(super) fn skipped(&mut self, name: &'static str, reason: &str) {
        self.checks.push(GateCheck::new(
            name,
            CheckStatus::Skipped,
            format!("skipped: {reason}"),
            None,
        ));
    }

    pub(super) fn not_applicable(&mut self, name: &'static str, reason: &str) {
        self.checks.push(GateCheck::new(
            name,
            CheckStatus::NotApplicable,
            format!("not applicable: {reason}"),
            None,
        ));
    }
}

#[derive(Serialize)]
pub(super) struct GateOutput<'a> {
    pub(super) checks: &'a [GateCheck],
}

#[derive(Serialize)]
pub(super) struct GateCheck {
    name: &'static str,
    status: CheckStatus,
    message: String,
    details: Option<Value>,
}

impl GateCheck {
    fn new(
        name: &'static str,
        status: CheckStatus,
        message: impl Into<String>,
        details: Option<Value>,
    ) -> Self {
        Self {
            name,
            status,
            message: message.into(),
            details,
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

pub(super) fn print_report(report: &GateReport, json: bool) -> Result<()> {
    let failed = report.failure_category.is_some();
    if json {
        let output_data = GateOutput {
            checks: &report.checks,
        };
        if failed {
            let mut stderr = std::io::stderr().lock();
            output::print_json_to("gate", output_data, &mut stderr)?;
        } else {
            output::print_json("gate", output_data)?;
        }
    } else if failed {
        let mut stderr = std::io::stderr().lock();
        print_human(&report.checks, &mut stderr)?;
    } else {
        let mut stdout = std::io::stdout().lock();
        print_human(&report.checks, &mut stdout)?;
    }
    Ok(())
}

fn print_human(checks: &[GateCheck], output: &mut impl Write) -> io::Result<()> {
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
