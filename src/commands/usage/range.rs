use crate::cli::UsageRangeArgs;
use crate::error::AixError;
use chrono::{Days, Local, NaiveDate};

#[derive(Clone, Copy)]
pub(super) struct UsageRange {
    pub(super) start: NaiveDate,
    pub(super) end: NaiveDate,
    pub(super) is_default: bool,
}

impl UsageRange {
    pub(super) fn from_args(args: &UsageRangeArgs, today: NaiveDate) -> Result<Self, AixError> {
        let since = args.since.as_deref();
        let start = args.start.as_deref();
        let end = args.end.as_deref();

        if since.is_some() && (start.is_some() || end.is_some()) {
            return Err(invalid_range(
                "--since cannot be combined with --start or --end",
            ));
        }

        match (since, start, end) {
            (Some(since), None, None) => {
                let days = parse_since(since)?;
                let start = today
                    .checked_sub_days(Days::new(days - 1))
                    .ok_or_else(|| invalid_range("the requested --since range is too large"))?;
                Ok(Self {
                    start,
                    end: today,
                    is_default: false,
                })
            }
            (None, Some(start), Some(end)) => {
                let start = parse_iso_date(start)?;
                let end = parse_iso_date(end)?;
                if end < start {
                    return Err(invalid_range("--end must be on or after --start"));
                }
                Ok(Self {
                    start,
                    end,
                    is_default: false,
                })
            }
            (None, None, None) => {
                let start = today
                    .checked_sub_days(Days::new(29))
                    .ok_or_else(|| invalid_range("the default 30-day range is out of bounds"))?;
                Ok(Self {
                    start,
                    end: today,
                    is_default: true,
                })
            }
            (None, _, _) => Err(invalid_range("--start and --end must be provided together")),
            (Some(_), _, _) => Err(invalid_range(
                "--since cannot be combined with --start or --end",
            )),
        }
    }

    pub(super) fn local_unix_millis_bounds(self) -> Result<(u64, u64), AixError> {
        let start = local_midnight(self.start)?.timestamp_millis();
        let next_day = self
            .end
            .checked_add_days(Days::new(1))
            .ok_or_else(|| invalid_range("the requested date range is out of bounds"))?;
        let end_exclusive = local_midnight(next_day)?.timestamp_millis();
        let end_inclusive = end_exclusive.saturating_sub(1);

        if end_inclusive < 0 {
            return Ok((1, 0));
        }
        Ok((start.max(0) as u64, end_inclusive as u64))
    }
}

fn local_midnight(date: NaiveDate) -> Result<chrono::DateTime<Local>, AixError> {
    date.and_hms_opt(0, 0, 0)
        .and_then(|datetime| {
            datetime
                .and_local_timezone(Local)
                .earliest()
                .or_else(|| datetime.and_local_timezone(Local).latest())
        })
        .ok_or_else(|| invalid_range("a date boundary cannot be represented in the local timezone"))
}

fn parse_since(value: &str) -> Result<u64, AixError> {
    let Some(days) = value.strip_suffix('d') else {
        return Err(invalid_range(
            "--since must be a positive whole-day duration such as 7d",
        ));
    };
    if days.is_empty() || !days.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid_range(
            "--since must be a positive whole-day duration such as 7d",
        ));
    }
    let days = days
        .parse::<u64>()
        .map_err(|_| invalid_range("--since day count is too large"))?;
    if days == 0 {
        return Err(invalid_range("--since must be greater than 0d"));
    }
    Ok(days)
}

fn parse_iso_date(value: &str) -> Result<NaiveDate, AixError> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| invalid_range("dates must use the inclusive YYYY-MM-DD format"))?;
    if date.format("%Y-%m-%d").to_string() != value {
        return Err(invalid_range(
            "dates must use the inclusive YYYY-MM-DD format",
        ));
    }
    Ok(date)
}

fn invalid_range(reason: &str) -> AixError {
    AixError::InvalidUsageRange {
        reason: reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, 30).unwrap()
    }

    #[test]
    fn default_range_is_thirty_calendar_days_including_today() {
        let range = UsageRange::from_args(&UsageRangeArgs::default(), fixed_today()).unwrap();
        assert_eq!(range.start.format("%Y-%m-%d").to_string(), "2026-09-01");
        assert_eq!(range.end.format("%Y-%m-%d").to_string(), "2026-09-30");
        assert!(range.is_default);
    }

    #[test]
    fn since_range_counts_calendar_days_including_today() {
        let args = UsageRangeArgs {
            since: Some("7d".to_string()),
            ..UsageRangeArgs::default()
        };
        let range = UsageRange::from_args(&args, fixed_today()).unwrap();
        assert_eq!(range.start.format("%Y-%m-%d").to_string(), "2026-09-24");
        assert_eq!(range.end.format("%Y-%m-%d").to_string(), "2026-09-30");
        assert!(!range.is_default);
    }

    #[test]
    fn explicit_dates_are_inclusive_and_iso_formatted() {
        let args = UsageRangeArgs {
            start: Some("2026-09-01".to_string()),
            end: Some("2026-09-30".to_string()),
            ..UsageRangeArgs::default()
        };
        let range = UsageRange::from_args(&args, fixed_today()).unwrap();
        assert_eq!(range.start.format("%Y-%m-%d").to_string(), "2026-09-01");
        assert_eq!(range.end.format("%Y-%m-%d").to_string(), "2026-09-30");
    }

    #[test]
    fn date_range_rejects_invalid_durations_dates_and_conflicts() {
        for value in ["0d", "7", "-1d", "1.5d", "7D", "18446744073709551616d"] {
            let args = UsageRangeArgs {
                since: Some(value.to_string()),
                ..UsageRangeArgs::default()
            };
            assert!(UsageRange::from_args(&args, fixed_today()).is_err());
        }

        let invalid_date = UsageRangeArgs {
            start: Some("2026-9-01".to_string()),
            end: Some("2026-09-02".to_string()),
            ..UsageRangeArgs::default()
        };
        assert!(UsageRange::from_args(&invalid_date, fixed_today()).is_err());

        let reversed = UsageRangeArgs {
            start: Some("2026-09-02".to_string()),
            end: Some("2026-09-01".to_string()),
            ..UsageRangeArgs::default()
        };
        assert!(UsageRange::from_args(&reversed, fixed_today()).is_err());

        let missing_end = UsageRangeArgs {
            start: Some("2026-09-01".to_string()),
            ..UsageRangeArgs::default()
        };
        assert!(UsageRange::from_args(&missing_end, fixed_today()).is_err());

        let conflict = UsageRangeArgs {
            since: Some("7d".to_string()),
            start: Some("2026-09-01".to_string()),
            end: Some("2026-09-02".to_string()),
        };
        assert!(UsageRange::from_args(&conflict, fixed_today()).is_err());
    }
}
