# Spend Color Output Design

**Date:** 2026-07-15
**Status:** Approved (amended 2026-07-15: discrete tiers replaced with a smooth gradient — see
"Color tiers" and "Implementation" below)

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

Based on `pct_used` (already computed in `print_human`), color is a continuous gradient
rather than a discrete jump. Four RGB stops anchor the gradient; the color at any
`pct_used` is linearly interpolated between the two nearest stops:

| `pct_used` | Color                  |
|------------|------------------------|
| `0%`       | green — `Rgb(0,200,0)`   |
| `50%`      | yellow — `Rgb(220,200,0)`|
| `75%`      | orange — `Rgb(255,165,0)` (no standard ANSI orange) |
| `100%`     | red — `Rgb(220,0,0)` (over budget) |

Values between stops interpolate per-channel (e.g. 63% is ~52% of the way from the 50%
stop to the 75% stop, giving `Rgb(238,182,0)`). `pct_used` is clamped to `[0, 100]` before
interpolating, matching the existing clamp in `print_human`.

Terminal-theme-derived colors (reading the user's actual configured ANSI palette) were
considered and rejected: there's no portable way to read back a terminal's live theme
colors without OSC 4 queries, which aren't universally supported and require raw-mode
reads with timeout/fallback handling — too much complexity for this feature.

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

const GRADIENT_STOPS: [(f64, Rgb); 4] = [
    (0.0, Rgb(0, 200, 0)),
    (50.0, Rgb(220, 200, 0)),
    (75.0, Rgb(255, 165, 0)),
    (100.0, Rgb(220, 0, 0)),
];

fn lerp_channel(a: u8, b: u8, t: f64) -> u8 {
    (a as f64 + (b as f64 - a as f64) * t).round() as u8
}

fn gradient_color(pct_used: f64) -> Rgb {
    let pct = pct_used.clamp(0.0, 100.0);
    let segment = GRADIENT_STOPS
        .windows(2)
        .find(|w| pct <= w[1].0)
        .unwrap_or(&GRADIENT_STOPS[GRADIENT_STOPS.len() - 2..]);
    let (p0, c0) = segment[0];
    let (p1, c1) = segment[1];
    let t = if p1 > p0 { (pct - p0) / (p1 - p0) } else { 0.0 };

    Rgb(
        lerp_channel(c0.0, c1.0, t),
        lerp_channel(c0.1, c1.1, t),
        lerp_channel(c0.2, c1.2, t),
    )
}
```

`print_human`'s budget branch builds the bar and label, then wraps just those two pieces:

```rust
let color = gradient_color(pct_used);
let bar = format!("[{}{}]", "█".repeat(filled), "░".repeat(BAR - filled));
let bar_colored = bar.if_supports_color(Stdout, |t| t.color(color)).to_string();
let label = format!("{pct_used:.0}% used");
let label_colored = label.if_supports_color(Stdout, |t| t.color(color)).to_string();
println!("{bar_colored}  {label_colored}");
```

`if_supports_color` checks both `NO_COLOR` and whether stdout is a TTY internally (via the
`supports-colors` feature), so no manual detection code is needed.

## Testing

- No unit test for ANSI byte output (brittle, low value). `gradient_color` itself is a pure
  function and gets unit tests covering the four exact stops, interpolated midpoints between
  each pair of adjacent stops, and out-of-range clamping.
- Manual verification: run `aix spend` against profiles at different usage levels (or a mock
  response) piped to a file vs. a real TTY, confirming color appears only in the TTY case and
  `NO_COLOR=1 aix spend` suppresses it.
