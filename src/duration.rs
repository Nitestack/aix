#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LiteLlmDuration {
    FixedSeconds(u64),
    CalendarMonths(u64),
}

pub(crate) fn parse_litellm_duration(value: &str) -> Option<LiteLlmDuration> {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > 128 {
        return None;
    }

    let digit_end = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digit_end == 0 || digit_end == bytes.len() {
        return None;
    }
    let amount = value[..digit_end].parse::<u64>().ok()?;
    if amount == 0 {
        return None;
    }

    let unit = &value[digit_end..];
    let duration = match unit {
        "s" => LiteLlmDuration::FixedSeconds(amount),
        "m" => LiteLlmDuration::FixedSeconds(amount.checked_mul(60)?),
        "h" => LiteLlmDuration::FixedSeconds(amount.checked_mul(60 * 60)?),
        "d" => LiteLlmDuration::FixedSeconds(amount.checked_mul(24 * 60 * 60)?),
        "w" => LiteLlmDuration::FixedSeconds(amount.checked_mul(7 * 24 * 60 * 60)?),
        "mo" => {
            // LiteLLM interprets months as calendar time, not a fixed number of seconds.
            amount.checked_mul(31 * 24 * 60 * 60)?;
            LiteLlmDuration::CalendarMonths(amount)
        }
        _ => return None,
    };
    Some(duration)
}

pub(crate) fn to_std_duration(value: &str) -> Option<std::time::Duration> {
    match parse_litellm_duration(value)? {
        LiteLlmDuration::FixedSeconds(seconds) => Some(std::time::Duration::from_secs(seconds)),
        // Treat calendar months as their shortest possible length so local
        // enforcement never extends beyond the configured LiteLLM maximum.
        LiteLlmDuration::CalendarMonths(months) => months
            .checked_mul(28 * 24 * 60 * 60)
            .map(std::time::Duration::from_secs),
    }
}

pub(crate) fn is_no_longer_than(requested: &str, maximum: &str) -> bool {
    let (Some(requested), Some(maximum)) = (
        parse_litellm_duration(requested),
        parse_litellm_duration(maximum),
    ) else {
        return false;
    };

    match (requested, maximum) {
        (LiteLlmDuration::FixedSeconds(requested), LiteLlmDuration::FixedSeconds(maximum)) => {
            requested <= maximum
        }
        (LiteLlmDuration::CalendarMonths(requested), LiteLlmDuration::CalendarMonths(maximum)) => {
            requested <= maximum
        }
        (LiteLlmDuration::CalendarMonths(requested), LiteLlmDuration::FixedSeconds(maximum)) => {
            requested
                .checked_mul(31 * 24 * 60 * 60)
                .is_some_and(|requested| requested <= maximum)
        }
        (LiteLlmDuration::FixedSeconds(requested), LiteLlmDuration::CalendarMonths(maximum)) => {
            // The shortest calendar month is 28 days. This conservative comparison
            // never widens a fixed-duration override beyond a calendar-month cap.
            maximum
                .checked_mul(28 * 24 * 60 * 60)
                .is_some_and(|maximum| requested <= maximum)
        }
    }
}
