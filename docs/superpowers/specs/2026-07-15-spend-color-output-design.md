# Spend Color Output Design

**Date:** 2026-07-15
**Status:** Approved

## Problem

`aix spend` prints a plain-text usage bar (`print_human` in `src/commands/spend.rs`).
There is no visual cue for how close a profile is to its budget — a user has to read the
percentage number to notice they're near or over budget.

## Goals

- Color the usage bar and `"NN% used"` label based on how much of the budget is consumed.
- Use a 4-tier scale so "near budget" (orange) is visually distinct from both "fine" (green/
  yellow) and "over budget" (red).
- Respect standard terminal conventions: no ANSI codes when stdout isn't a TTY, and honor
  `NO_COLOR`.
- `--json` output is unaffected (it already never emits ANSI codes).

## Non-goals

- Coloring the dollar-amount summary line or the "no budget set" case — there's no ratio to
  color by.
- A `--no-color` / `--color` CLI flag — TTY + `NO_COLOR` detection is sufficient for now.
- Configurable thresholds/colors via `aix.toml`.

## Color tiers

Based on `pct_used` (already computed in `print_human`):

| Range        | Color                  |
|--------------|------------------------|
| `< 50%`      | green                  |
| `50% – 74%`  | yellow                 |
| `75% – 99%`  | orange (`Rgb(255,165,0)`, no standard ANSI orange) |
| `>= 100%`    | red (over budget)      |

## What gets colored

Only two things, both in the second output line:

- The filled (`█`) portion of the progress bar.
- The `"{pct_used:.0}% used"` label.

The empty (`░`) portion of the bar and the entire first line (`$X of $Y ... available/over
budget by ...`) stay in the default terminal color.

## Implementation

Add to `Cargo.toml`:

```toml
owo-colors = { version = "4", features = ["supports-colors"] }
```

Pure-Rust, no C build dependencies (consistent with the cross-platform rule in
`.claude/rules/aix-rust.md`).

In `src/commands/spend.rs`:

```rust
use owo_colors::{OwoColorize, Rgb, Stream::Stdout};

fn tier_color(pct_used: f64) -> Rgb {
    match pct_used {
        p if p < 50.0 => Rgb(0, 200, 0),
        p if p < 75.0 => Rgb(220, 200, 0),
        p if p < 100.0 => Rgb(255, 165, 0),
        _ => Rgb(220, 0, 0),
    }
}
```

`print_human`'s budget branch builds the bar and label, then wraps just those two pieces:

```rust
let color = tier_color(pct_used);
let bar = format!("[{}{}]", "█".repeat(filled), "░".repeat(BAR - filled));
let bar_colored = bar.if_supports_color(Stdout, |t| t.color(color)).to_string();
let label = format!("{pct_used:.0}% used");
let label_colored = label.if_supports_color(Stdout, |t| t.color(color)).to_string();
println!("{bar_colored}  {label_colored}");
```

`if_supports_color` checks both `NO_COLOR` and whether stdout is a TTY internally (via the
`supports-colors` feature), so no manual detection code is needed.

## Testing

- No unit test for ANSI byte output (brittle, low value). `tier_color` itself is a pure
  function and gets a unit test covering the four boundary cases (49%, 50%, 74%, 75%, 99%,
  100%).
- Manual verification: run `aix spend` against profiles at different usage levels (or a mock
  response) piped to a file vs. a real TTY, confirming color appears only in the TTY case and
  `NO_COLOR=1 aix spend` suppresses it.
