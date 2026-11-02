use crate::gateway::BudgetLayer;
use owo_colors::{OwoColorize, Rgb, Stream::Stdout};

const BAR_WIDTH: usize = 40;
const GRADIENT_STOPS: [(f64, Rgb); 4] = [
    (0.0, Rgb(0, 200, 0)),
    (50.0, Rgb(220, 200, 0)),
    (75.0, Rgb(255, 165, 0)),
    (100.0, Rgb(220, 0, 0)),
];

pub(super) fn spend_lines(spend: f64, max_budget: Option<f64>, cache_suffix: &str) -> Vec<String> {
    let Some(budget) = max_budget else {
        return vec![format!("${spend:.2} spent  (no budget set){cache_suffix}")];
    };
    let remaining = budget - spend;
    let percent_used = if budget > 0.0 {
        (spend / budget * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    let percent_remaining = 100.0 - percent_used;
    let summary = if remaining < 0.0 {
        format!(
            "${spend:.2} of ${budget:.2}  ·  over budget by ${:.2}{cache_suffix}",
            -remaining
        )
    } else {
        format!(
            "${spend:.2} of ${budget:.2}  ·  ${remaining:.2} available ({percent_remaining:.0}%){cache_suffix}"
        )
    };
    vec![summary, render_progress(percent_used)]
}

pub(crate) fn budget_lines(
    key_spent: f64,
    remaining: Option<f64>,
    limited_by: Option<&str>,
    layers: &[BudgetLayer],
    expires_at: Option<&str>,
    cache_suffix: &str,
) -> Vec<String> {
    let (available_label, available) = match remaining {
        Some(amount) if amount < 0.0 => ("Over budget", money(-amount)),
        _ => ("Available", remaining_label(remaining)),
    };
    let limiting = limited_by.map_or(String::new(), |scope| format!(" · limited by {scope}"));
    let mut lines = vec![
        format!("{:<12}{}{cache_suffix}", "Key spend", money(key_spent)),
        format!("{available_label:<12}{available}{limiting}"),
    ];

    if !layers.is_empty() {
        let rows: Vec<_> = layers
            .iter()
            .map(|layer| {
                let label = match layer.scope.as_str() {
                    "key" => "Key",
                    "personal" => "Personal",
                    "project" => "Project",
                    "tenant" => "Tenant",
                    other => other,
                };
                let marker = if limited_by == Some(layer.scope.as_str()) {
                    " *"
                } else {
                    ""
                };
                let limit = layer.budget_usd.map_or("unlimited".into(), money);
                (
                    format!("{label}{marker}"),
                    format!("{} / {limit}", money(layer.spent_usd)),
                    remaining_label(layer.remaining_usd),
                    layer,
                )
            })
            .collect();
        let label_width = rows.iter().map(|row| row.0.len()).max().unwrap().max(6);
        let amount_width = rows.iter().map(|row| row.1.len()).max().unwrap().max(13);
        lines.push(String::new());
        lines.push(format!(
            "{:<label_width$}  {:<amount_width$}  Remaining",
            "Budget", "Spent / Limit"
        ));
        for (label, amounts, remaining, layer) in rows {
            lines.push(format!(
                "{label:<label_width$}  {amounts:<amount_width$}  {remaining}"
            ));
            if let Some(limit) = layer.budget_usd {
                lines.push(format!("  {}", progress_bar(layer.spent_usd, limit)));
            }
        }
        if layers
            .iter()
            .any(|layer| limited_by == Some(layer.scope.as_str()))
        {
            lines.push("* Limiting budget".into());
        }
    }

    if let Some(expires_at) = expires_at {
        let expiry = chrono::DateTime::parse_from_rfc3339(expires_at)
            .map(|time| {
                time.with_timezone(&chrono::Utc)
                    .format("%Y-%m-%d %H:%M UTC")
                    .to_string()
            })
            .unwrap_or_else(|_| expires_at.to_owned());
        lines.push(format!("{:<12}{expiry}", "Expires"));
    }
    lines
}

fn money(amount: f64) -> String {
    if amount > 0.0 && amount < 0.000_001 {
        "<$0.000001".into()
    } else if amount > 0.0 && amount < 0.01 {
        format!("${amount:.6}").trim_end_matches('0').to_string()
    } else {
        format!("${amount:.2}")
    }
}

fn remaining_label(amount: Option<f64>) -> String {
    match amount {
        Some(amount) if amount < 0.0 => format!("over by {}", money(-amount)),
        Some(amount) => money(amount),
        None => "unlimited".into(),
    }
}

fn progress_bar(spent: f64, limit: f64) -> String {
    let fraction = if limit > 0.0 {
        (spent / limit).clamp(0.0, 1.0)
    } else {
        1.0
    };
    render_progress(fraction * 100.0)
}

fn render_progress(percent: f64) -> String {
    let filled = (percent / 100.0 * BAR_WIDTH as f64).round() as usize;
    let used: String = (0..filled)
        .map(|index| {
            "█"
                .if_supports_color(Stdout, |text| {
                    text.color(gradient_color(
                        index as f64 / (BAR_WIDTH - 1) as f64 * 100.0,
                    ))
                })
                .to_string()
        })
        .collect();
    let empty = "░".repeat(BAR_WIDTH - filled);
    let label = format!("{percent:.0}% used")
        .if_supports_color(Stdout, |text| text.color(gradient_color(percent)))
        .to_string();
    format!("[{used}{empty}]  {label}")
}

fn gradient_color(percent: f64) -> Rgb {
    let percent = percent.clamp(0.0, 100.0);
    let segment = GRADIENT_STOPS
        .windows(2)
        .find(|stops| percent <= stops[1].0)
        .unwrap_or(&GRADIENT_STOPS[GRADIENT_STOPS.len() - 2..]);
    let (start, from) = segment[0];
    let (end, to) = segment[1];
    let fraction = (percent - start) / (end - start);
    let channel = |from: u8, to: u8| {
        (f64::from(from) + (f64::from(to) - f64::from(from)) * fraction).round() as u8
    };
    Rgb(
        channel(from.0, to.0),
        channel(from.1, to.1),
        channel(from.2, to.2),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_spend_summary_keeps_two_decimals_and_one_bar() {
        let lines = spend_lines(41.53, Some(100.0), "");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "$41.53 of $100.00  ·  $58.47 available (58%)");
        assert!(lines[1].contains("42% used"));
        assert_eq!(
            spend_lines(0.000_312_4, None, ""),
            ["$0.00 spent  (no budget set)"]
        );
    }

    #[test]
    fn original_spend_summary_keeps_over_budget_and_cache_wording() {
        let lines = spend_lines(25.0, Some(20.0), "  ·  cached 1m ago");
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0],
            "$25.00 of $20.00  ·  over budget by $5.00  ·  cached 1m ago"
        );
        assert_eq!(lines[1].matches('█').count(), BAR_WIDTH);
        assert!(lines[1].contains("100% used"));
        assert_eq!(
            spend_lines(25.0, None, "  ·  cached 1m ago"),
            ["$25.00 spent  (no budget set)  ·  cached 1m ago"]
        );
    }

    #[test]
    fn original_zero_budget_rendering_is_unchanged() {
        let lines = spend_lines(0.0, Some(0.0), "");
        assert_eq!(lines[0], "$0.00 of $0.00  ·  $0.00 available (100%)");
        assert_eq!(lines[1].matches('░').count(), BAR_WIDTH);
        assert!(lines[1].contains("0% used"));
    }

    fn layer(limit: Option<f64>, spent: f64, remaining: Option<f64>) -> BudgetLayer {
        BudgetLayer {
            scope: "key".into(),
            budget_usd: limit,
            spent_usd: spent,
            remaining_usd: remaining,
            period_start: "2026-11-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn progress_bar_keeps_the_original_gradient() {
        assert_eq!(gradient_color(0.0), Rgb(0, 200, 0));
        assert_eq!(gradient_color(25.0), Rgb(110, 200, 0));
        assert_eq!(gradient_color(50.0), Rgb(220, 200, 0));
        assert_eq!(gradient_color(75.0), Rgb(255, 165, 0));
        assert_eq!(gradient_color(100.0), Rgb(220, 0, 0));
        assert_eq!(gradient_color(150.0), Rgb(220, 0, 0));
    }

    #[test]
    fn zero_and_exceeded_budgets_show_a_full_bar_and_clear_amounts() {
        for (spent, limit, remaining, expected) in [
            (0.0, 0.0, 0.0, "$0.00 / $0.00"),
            (25.0, 20.0, -5.0, "over by $5.00"),
        ] {
            let text = budget_lines(
                spent,
                Some(remaining),
                Some("key"),
                &[layer(Some(limit), spent, Some(remaining))],
                None,
                "",
            )
            .join("\n");
            assert!(text.contains(expected), "{text}");
            assert!(text.contains("100% used"), "{text}");
            assert_eq!(text.matches('█').count(), BAR_WIDTH);
            assert!(!text.contains("NaN"));
        }
    }

    #[test]
    fn unlimited_budget_has_no_fabricated_percentage_or_progress_bar() {
        let text = budget_lines(
            1.0,
            None,
            None,
            &[layer(None, 1.0, None)],
            None,
            " · cached 1m ago",
        )
        .join("\n");
        assert!(text.contains("Available   unlimited"));
        assert!(text.contains("$1.00 / unlimited"));
        assert!(text.contains("cached 1m ago"));
        assert!(!text.contains('%'));
        assert!(!text.contains('░'));
        assert!(!text.contains('█'));
    }

    #[test]
    fn missing_layers_do_not_invent_a_budget_or_percentage() {
        let text = budget_lines(41.53, Some(12.0), Some("project"), &[], None, "").join("\n");
        assert!(text.contains("$41.53"));
        assert!(text.contains("$12.00"));
        assert!(!text.contains("$53.53"));
        assert!(!text.contains('%'));
    }

    #[test]
    fn small_nonzero_spend_is_not_rounded_to_zero() {
        assert_eq!(money(0.000_312_4), "$0.000312");
        assert_eq!(money(0.005), "$0.005");
        assert_eq!(money(0.000_000_1), "<$0.000001");
        assert_eq!(money(0.0), "$0.00");
        assert_eq!(money(12.0), "$12.00");
    }
}
