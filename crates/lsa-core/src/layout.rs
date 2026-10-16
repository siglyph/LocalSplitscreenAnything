//! Layout engine: decides where each player's window goes.
//!
//! A [`Layout`] is a list of [`Viewport`]s whose rectangles are normalised (0..1) relative to a
//! monitor, so a saved layout survives resolution changes and can be edited independently of
//! the machine it was made on. [`preset`] generates the built-in arrangements for 2–4 players,
//! and [`Layout::resolve`] turns a layout into pixel rectangles for the current monitors.

use std::fmt;

use lsa_proto::PixelRect;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const EPS: f64 = 1e-6;

/// A physical display, in virtual-desktop pixel coordinates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Monitor {
    /// Stable identifier (on Windows the device name, e.g. `\\.\DISPLAY1`).
    pub id: String,
    /// Friendly name for the UI.
    pub name: String,
    pub rect: PixelRect,
    pub primary: bool,
    /// DPI scale factor (1.0 = 100%).
    #[serde(default = "default_scale")]
    pub scale: f64,
}

fn default_scale() -> f64 {
    1.0
}

impl Monitor {
    pub fn simple(id: &str, x: i32, y: i32, width: i32, height: i32, primary: bool) -> Self {
        Self {
            id: id.to_string(),
            name: id.to_string(),
            rect: PixelRect::new(x, y, width, height),
            primary,
            scale: 1.0,
        }
    }

    pub fn aspect(&self) -> f64 {
        self.rect.width as f64 / self.rect.height.max(1) as f64
    }
}

/// A rectangle normalised to a monitor: all four values are fractions in 0..=1.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NormRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl NormRect {
    pub const FULL: NormRect = NormRect {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    };

    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    /// Convert to pixels inside `monitor`. Edges are rounded individually so adjacent
    /// viewports share an edge with no gap and no overlap.
    pub fn to_pixels(&self, monitor: &PixelRect) -> PixelRect {
        let fx = |f: f64| (f * monitor.width as f64).round() as i32;
        let fy = |f: f64| (f * monitor.height as f64).round() as i32;
        let x0 = fx(self.x);
        let x1 = fx(self.x + self.w);
        let y0 = fy(self.y);
        let y1 = fy(self.y + self.h);
        PixelRect::new(
            monitor.x + x0,
            monitor.y + y0,
            (x1 - x0).max(1),
            (y1 - y0).max(1),
        )
    }

    pub fn overlaps(&self, o: &NormRect) -> bool {
        self.x < o.x + o.w - EPS
            && o.x < self.x + self.w - EPS
            && self.y < o.y + o.h - EPS
            && o.y < self.y + self.h - EPS
    }

    pub fn is_within_unit(&self) -> bool {
        self.x >= -EPS
            && self.y >= -EPS
            && self.w > EPS
            && self.h > EPS
            && self.x + self.w <= 1.0 + EPS
            && self.y + self.h <= 1.0 + EPS
    }

    pub fn contains_point(&self, px: f64, py: f64) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    /// Aspect ratio of this viewport when shown on a monitor with `monitor_aspect`.
    pub fn aspect_on(&self, monitor_aspect: f64) -> f64 {
        (self.w / self.h) * monitor_aspect
    }
}

/// One player's window slot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Viewport {
    /// 1-based player number.
    pub player: u8,
    /// Index into the monitor list the layout was resolved against.
    pub monitor: usize,
    pub rect: NormRect,
}

/// A complete arrangement for `player_count` players.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub name: String,
    pub player_count: u8,
    pub viewports: Vec<Viewport>,
}

/// A viewport turned into pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedViewport {
    pub player: u8,
    pub monitor: usize,
    pub rect: PixelRect,
}

#[derive(Debug, Error, PartialEq)]
pub enum LayoutError {
    #[error("player count must be 1..=4, got {0}")]
    PlayerCount(u8),
    #[error("preset {preset} needs {needed} monitor(s) but only {available} are connected")]
    NotEnoughMonitors {
        preset: PresetKind,
        needed: usize,
        available: usize,
    },
    #[error("preset {0} does not support {1} players")]
    UnsupportedPlayerCount(PresetKind, u8),
    #[error("layout '{0}' is invalid: {1}")]
    Invalid(String, String),
    #[error("unknown preset '{0}' (try: {1})")]
    UnknownPreset(String, String),
    #[error("layout references monitor #{0} but only {1} monitor(s) are connected")]
    MissingMonitor(usize, usize),
}

/// Built-in arrangements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PresetKind {
    /// Two players side by side (alias of `columns` for 2).
    LeftRight,
    /// Two players stacked (alias of `rows` for 2).
    TopBottom,
    /// 2×2 grid; with 3 players one cell stays empty.
    Grid,
    /// N equal columns.
    Columns,
    /// N equal rows.
    Rows,
    /// Player 1 gets a big area on the left, the others stack on the right.
    BigLeft,
    /// Player 1 big on the right, others stacked on the left.
    BigRight,
    /// Player 1 big on top, others in a row below.
    BigTop,
    /// Player 1 big at the bottom, others in a row above.
    BigBottom,
    /// Each player gets a whole monitor.
    OnePerMonitor,
    /// Two players per monitor, side by side.
    TwoPerMonitor,
    /// Two players per monitor, stacked.
    TwoPerMonitorStacked,
}

impl PresetKind {
    pub const ALL: [PresetKind; 12] = [
        PresetKind::LeftRight,
        PresetKind::TopBottom,
        PresetKind::Grid,
        PresetKind::Columns,
        PresetKind::Rows,
        PresetKind::BigLeft,
        PresetKind::BigRight,
        PresetKind::BigTop,
        PresetKind::BigBottom,
        PresetKind::OnePerMonitor,
        PresetKind::TwoPerMonitor,
        PresetKind::TwoPerMonitorStacked,
    ];

    pub fn id(&self) -> &'static str {
        match self {
            PresetKind::LeftRight => "left-right",
            PresetKind::TopBottom => "top-bottom",
            PresetKind::Grid => "grid",
            PresetKind::Columns => "columns",
            PresetKind::Rows => "rows",
            PresetKind::BigLeft => "big-left",
            PresetKind::BigRight => "big-right",
            PresetKind::BigTop => "big-top",
            PresetKind::BigBottom => "big-bottom",
            PresetKind::OnePerMonitor => "one-per-monitor",
            PresetKind::TwoPerMonitor => "two-per-monitor",
            PresetKind::TwoPerMonitorStacked => "two-per-monitor-stacked",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            PresetKind::LeftRight => "Two players side by side",
            PresetKind::TopBottom => "Two players stacked",
            PresetKind::Grid => "2x2 grid (one empty cell with 3 players)",
            PresetKind::Columns => "Equal columns",
            PresetKind::Rows => "Equal rows",
            PresetKind::BigLeft => "Player 1 big on the left, others stacked on the right",
            PresetKind::BigRight => "Player 1 big on the right, others stacked on the left",
            PresetKind::BigTop => "Player 1 big on top, others in a row below",
            PresetKind::BigBottom => "Player 1 big at the bottom, others in a row above",
            PresetKind::OnePerMonitor => "One player per monitor",
            PresetKind::TwoPerMonitor => "Two players per monitor, side by side",
            PresetKind::TwoPerMonitorStacked => "Two players per monitor, stacked",
        }
    }

    /// Parse a preset id such as `grid` or `big-left` (also accepts `big_left` and `bigleft`).
    pub fn parse(s: &str) -> Result<Self, LayoutError> {
        let norm: String = s
            .trim()
            .to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        for kind in PresetKind::ALL {
            let id: String = kind
                .id()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect();
            if id == norm {
                return Ok(kind);
            }
        }
        // Convenience aliases.
        match norm.as_str() {
            "lr" | "sidebyside" | "horizontal" | "vertical2" => Ok(PresetKind::LeftRight),
            "tb" | "stacked" | "vertical" => Ok(PresetKind::TopBottom),
            "quad" | "2x2" => Ok(PresetKind::Grid),
            _ => Err(LayoutError::UnknownPreset(
                s.to_string(),
                PresetKind::ALL
                    .iter()
                    .map(|k| k.id())
                    .collect::<Vec<_>>()
                    .join(", "),
            )),
        }
    }

    /// Does this preset make sense for the given player count and monitor count?
    pub fn supports(&self, players: u8, monitors: usize) -> bool {
        let p = players as usize;
        match self {
            PresetKind::LeftRight | PresetKind::TopBottom => p == 2,
            PresetKind::Grid => (2..=4).contains(&p),
            PresetKind::Columns | PresetKind::Rows => (2..=4).contains(&p),
            PresetKind::BigLeft
            | PresetKind::BigRight
            | PresetKind::BigTop
            | PresetKind::BigBottom => (3..=4).contains(&p),
            PresetKind::OnePerMonitor => p >= 1 && monitors >= p,
            PresetKind::TwoPerMonitor | PresetKind::TwoPerMonitorStacked => {
                p >= 2 && monitors >= p.div_ceil(2) && monitors >= 2
            }
        }
    }
}

impl fmt::Display for PresetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

/// Which 2×2 cell stays empty for a 3-player grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PresetOptions {
    pub grid_empty_cell: Corner,
}

/// Presets usable with `players` players on `monitors` monitors.
pub fn available_presets(players: u8, monitors: usize) -> Vec<PresetKind> {
    PresetKind::ALL
        .iter()
        .copied()
        .filter(|k| k.supports(players, monitors))
        .collect()
}

/// Generate a built-in layout.
pub fn preset(
    kind: PresetKind,
    players: u8,
    monitors: &[Monitor],
    options: &PresetOptions,
) -> Result<Layout, LayoutError> {
    if !(1..=4).contains(&players) {
        return Err(LayoutError::PlayerCount(players));
    }
    let p = players as usize;
    let monitor_count = monitors.len().max(1);
    let needed_monitors = match kind {
        PresetKind::OnePerMonitor => p,
        PresetKind::TwoPerMonitor | PresetKind::TwoPerMonitorStacked => p.div_ceil(2).max(2),
        _ => 1,
    };
    if monitors.len() < needed_monitors {
        return Err(LayoutError::NotEnoughMonitors {
            preset: kind,
            needed: needed_monitors,
            available: monitors.len(),
        });
    }
    if !kind.supports(players, monitor_count) && players != 1 {
        return Err(LayoutError::UnsupportedPlayerCount(kind, players));
    }

    let mut viewports: Vec<Viewport> = Vec::with_capacity(p);
    let mut push = |player: usize, monitor: usize, rect: NormRect| {
        viewports.push(Viewport {
            player: player as u8,
            monitor,
            rect,
        });
    };

    if players == 1 {
        push(1, 0, NormRect::FULL);
        return Ok(Layout {
            name: kind.id().to_string(),
            player_count: 1,
            viewports,
        });
    }

    match kind {
        PresetKind::LeftRight | PresetKind::Columns => {
            let w = 1.0 / p as f64;
            for i in 0..p {
                push(i + 1, 0, NormRect::new(i as f64 * w, 0.0, w, 1.0));
            }
        }
        PresetKind::TopBottom | PresetKind::Rows => {
            let h = 1.0 / p as f64;
            for i in 0..p {
                push(i + 1, 0, NormRect::new(0.0, i as f64 * h, 1.0, h));
            }
        }
        PresetKind::Grid => {
            if p == 2 {
                push(1, 0, NormRect::new(0.0, 0.0, 0.5, 1.0));
                push(2, 0, NormRect::new(0.5, 0.0, 0.5, 1.0));
            } else {
                let cells = [
                    (Corner::TopLeft, NormRect::new(0.0, 0.0, 0.5, 0.5)),
                    (Corner::TopRight, NormRect::new(0.5, 0.0, 0.5, 0.5)),
                    (Corner::BottomLeft, NormRect::new(0.0, 0.5, 0.5, 0.5)),
                    (Corner::BottomRight, NormRect::new(0.5, 0.5, 0.5, 0.5)),
                ];
                let mut player = 1;
                for (corner, rect) in cells {
                    if p == 3 && corner == options.grid_empty_cell {
                        continue;
                    }
                    push(player, 0, rect);
                    player += 1;
                }
            }
        }
        PresetKind::BigLeft | PresetKind::BigRight => {
            // k small viewports stacked in a column of width 1/k keep the monitor's aspect
            // ratio; the big viewport takes the rest.
            let k = p - 1;
            let small_w = 1.0 / k as f64;
            let big_w = 1.0 - small_w;
            let (big_x, small_x) = if kind == PresetKind::BigLeft {
                (0.0, big_w)
            } else {
                (small_w, 0.0)
            };
            push(1, 0, NormRect::new(big_x, 0.0, big_w, 1.0));
            for i in 0..k {
                push(
                    i + 2,
                    0,
                    NormRect::new(small_x, i as f64 * small_w, small_w, small_w),
                );
            }
        }
        PresetKind::BigTop | PresetKind::BigBottom => {
            let k = p - 1;
            let small_h = 1.0 / k as f64;
            let big_h = 1.0 - small_h;
            let (big_y, small_y) = if kind == PresetKind::BigTop {
                (0.0, big_h)
            } else {
                (small_h, 0.0)
            };
            push(1, 0, NormRect::new(0.0, big_y, 1.0, big_h));
            for i in 0..k {
                push(
                    i + 2,
                    0,
                    NormRect::new(i as f64 * small_h, small_y, small_h, small_h),
                );
            }
        }
        PresetKind::OnePerMonitor => {
            for i in 0..p {
                push(i + 1, i, NormRect::FULL);
            }
        }
        PresetKind::TwoPerMonitor | PresetKind::TwoPerMonitorStacked => {
            let stacked = kind == PresetKind::TwoPerMonitorStacked;
            for i in 0..p {
                let monitor = i / 2;
                let second = i % 2 == 1;
                let rect = match (stacked, second) {
                    (false, false) => NormRect::new(0.0, 0.0, 0.5, 1.0),
                    (false, true) => NormRect::new(0.5, 0.0, 0.5, 1.0),
                    (true, false) => NormRect::new(0.0, 0.0, 1.0, 0.5),
                    (true, true) => NormRect::new(0.0, 0.5, 1.0, 0.5),
                };
                // A lone last player on an otherwise empty monitor gets the whole monitor.
                let rect = if !second && i + 1 == p {
                    NormRect::FULL
                } else {
                    rect
                };
                push(i + 1, monitor, rect);
            }
        }
    }

    let layout = Layout {
        name: kind.id().to_string(),
        player_count: players,
        viewports,
    };
    layout.validate(monitor_count)?;
    Ok(layout)
}

impl Layout {
    /// Check that every player 1..=player_count appears exactly once, every rect lies inside
    /// its monitor, no two viewports on the same monitor overlap, and all monitors exist.
    pub fn validate(&self, monitor_count: usize) -> Result<(), LayoutError> {
        let invalid = |msg: String| LayoutError::Invalid(self.name.clone(), msg);
        if !(1..=4).contains(&self.player_count) {
            return Err(LayoutError::PlayerCount(self.player_count));
        }
        if self.viewports.len() != self.player_count as usize {
            return Err(invalid(format!(
                "expected {} viewports, found {}",
                self.player_count,
                self.viewports.len()
            )));
        }
        for player in 1..=self.player_count {
            let n = self.viewports.iter().filter(|v| v.player == player).count();
            if n != 1 {
                return Err(invalid(format!("player {player} appears {n} times")));
            }
        }
        for v in &self.viewports {
            if v.monitor >= monitor_count {
                return Err(LayoutError::MissingMonitor(v.monitor, monitor_count));
            }
            if !v.rect.is_within_unit() {
                return Err(invalid(format!(
                    "player {} viewport leaves the monitor",
                    v.player
                )));
            }
        }
        for (i, a) in self.viewports.iter().enumerate() {
            for b in &self.viewports[i + 1..] {
                if a.monitor == b.monitor && a.rect.overlaps(&b.rect) {
                    return Err(invalid(format!(
                        "players {} and {} overlap on monitor {}",
                        a.player, b.player, a.monitor
                    )));
                }
            }
        }
        Ok(())
    }

    /// Turn the layout into pixel rectangles for the given monitors.
    pub fn resolve(&self, monitors: &[Monitor]) -> Result<Vec<ResolvedViewport>, LayoutError> {
        self.validate(monitors.len())?;
        let mut out: Vec<ResolvedViewport> = self
            .viewports
            .iter()
            .map(|v| ResolvedViewport {
                player: v.player,
                monitor: v.monitor,
                rect: v.rect.to_pixels(&monitors[v.monitor].rect),
            })
            .collect();
        out.sort_by_key(|r| r.player);
        Ok(out)
    }

    pub fn viewport_for(&self, player: u8) -> Option<&Viewport> {
        self.viewports.iter().find(|v| v.player == player)
    }

    /// Swap the positions of two players (used by the "swap" hotkey).
    pub fn swap_players(&mut self, a: u8, b: u8) {
        let ia = self.viewports.iter().position(|v| v.player == a);
        let ib = self.viewports.iter().position(|v| v.player == b);
        if let (Some(ia), Some(ib)) = (ia, ib) {
            self.viewports[ia].player = b;
            self.viewports[ib].player = a;
        }
    }

    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    pub fn from_toml_str(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// Players whose viewport aspect ratio is far from the monitor's (likely letterboxing or a
    /// cramped UI). Returns `(player, aspect)`.
    pub fn odd_aspect_viewports(&self, monitors: &[Monitor]) -> Vec<(u8, f64)> {
        self.viewports
            .iter()
            .filter_map(|v| {
                let m = monitors.get(v.monitor)?;
                let aspect = v.rect.aspect_on(m.aspect());
                let ratio = aspect / m.aspect();
                if !(0.6..=1.7).contains(&ratio) {
                    Some((v.player, aspect))
                } else {
                    None
                }
            })
            .collect()
    }
}

/// Draw the layout as ASCII art, one box per monitor, for the CLI.
pub fn ascii_preview(layout: &Layout, monitors: &[Monitor]) -> String {
    const COLS: usize = 40;
    const ROWS: usize = 12;
    let monitor_count = monitors.len().max(1);
    let mut boxes: Vec<Vec<String>> = Vec::new();
    for m in 0..monitor_count {
        let mut lines = Vec::with_capacity(ROWS + 3);
        let label = monitors
            .get(m)
            .map(|mon| format!("{} {}x{}", mon.name, mon.rect.width, mon.rect.height))
            .unwrap_or_else(|| format!("monitor {m}"));
        lines.push(format!("{label:<width$}", width = COLS + 2));
        lines.push(format!("+{}+", "-".repeat(COLS)));
        for r in 0..ROWS {
            let mut line = String::with_capacity(COLS + 2);
            line.push('|');
            for c in 0..COLS {
                let px = (c as f64 + 0.5) / COLS as f64;
                let py = (r as f64 + 0.5) / ROWS as f64;
                let ch = layout
                    .viewports
                    .iter()
                    .find(|v| v.monitor == m && v.rect.contains_point(px, py))
                    .map(|v| char::from_digit(v.player as u32, 10).unwrap_or('?'))
                    .unwrap_or('.');
                line.push(ch);
            }
            line.push('|');
            lines.push(line);
        }
        lines.push(format!("+{}+", "-".repeat(COLS)));
        boxes.push(lines);
    }
    let height = boxes.iter().map(|b| b.len()).max().unwrap_or(0);
    let mut out = String::new();
    for row in 0..height {
        let parts: Vec<&str> = boxes
            .iter()
            .map(|b| b.get(row).map(String::as_str).unwrap_or(""))
            .collect();
        out.push_str(parts.join("  ").trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_monitor() -> Vec<Monitor> {
        vec![Monitor::simple("M1", 0, 0, 1920, 1080, true)]
    }

    fn two_monitors() -> Vec<Monitor> {
        vec![
            Monitor::simple("M1", 0, 0, 1920, 1080, true),
            Monitor::simple("M2", 1920, 0, 2560, 1440, false),
        ]
    }

    fn area(layout: &Layout) -> f64 {
        layout.viewports.iter().map(|v| v.rect.w * v.rect.h).sum()
    }

    #[test]
    fn every_single_monitor_preset_is_valid_for_supported_counts() {
        let mons = one_monitor();
        for kind in PresetKind::ALL {
            for players in 2..=4u8 {
                if !kind.supports(players, 1) {
                    continue;
                }
                let layout = preset(kind, players, &mons, &PresetOptions::default())
                    .unwrap_or_else(|e| panic!("{kind} with {players}: {e}"));
                assert_eq!(layout.viewports.len(), players as usize, "{kind} {players}");
                layout.validate(1).unwrap();
            }
        }
    }

    #[test]
    fn full_coverage_presets_fill_the_monitor() {
        let mons = one_monitor();
        for (kind, players) in [
            (PresetKind::LeftRight, 2),
            (PresetKind::TopBottom, 2),
            (PresetKind::Columns, 3),
            (PresetKind::Rows, 4),
            (PresetKind::Grid, 4),
            (PresetKind::BigLeft, 3),
            (PresetKind::BigLeft, 4),
            (PresetKind::BigTop, 4),
            (PresetKind::BigRight, 3),
            (PresetKind::BigBottom, 3),
        ] {
            let layout = preset(kind, players, &mons, &PresetOptions::default()).unwrap();
            assert!(
                (area(&layout) - 1.0).abs() < 1e-9,
                "{kind} {players} covers {}",
                area(&layout)
            );
        }
    }

    #[test]
    fn grid_with_three_players_leaves_requested_cell_empty() {
        let mons = one_monitor();
        let opts = PresetOptions {
            grid_empty_cell: Corner::TopLeft,
        };
        let layout = preset(PresetKind::Grid, 3, &mons, &opts).unwrap();
        assert!((area(&layout) - 0.75).abs() < 1e-9);
        assert!(layout
            .viewports
            .iter()
            .all(|v| !(v.rect.x == 0.0 && v.rect.y == 0.0)));
        assert_eq!(
            layout.viewport_for(1).unwrap().rect,
            NormRect::new(0.5, 0.0, 0.5, 0.5)
        );
    }

    #[test]
    fn big_left_small_cells_keep_monitor_aspect() {
        let mons = one_monitor();
        let layout = preset(PresetKind::BigLeft, 4, &mons, &PresetOptions::default()).unwrap();
        let big = layout.viewport_for(1).unwrap();
        assert!((big.rect.w - 2.0 / 3.0).abs() < 1e-9);
        for p in 2..=4 {
            let v = layout.viewport_for(p).unwrap();
            assert!((v.rect.aspect_on(mons[0].aspect()) - mons[0].aspect()).abs() < 1e-9);
        }
    }

    #[test]
    fn resolve_rounds_edges_without_gaps() {
        let mons = vec![Monitor::simple("M1", 0, 0, 1921, 1081, true)];
        let layout = preset(PresetKind::Columns, 3, &mons, &PresetOptions::default()).unwrap();
        let r = layout.resolve(&mons).unwrap();
        assert_eq!(r[0].rect.x, 0);
        assert_eq!(r[0].rect.right(), r[1].rect.x);
        assert_eq!(r[1].rect.right(), r[2].rect.x);
        assert_eq!(r[2].rect.right(), 1921);
        assert!(r.iter().all(|v| v.rect.height == 1081));
    }

    #[test]
    fn one_per_monitor_needs_enough_monitors() {
        let err = preset(
            PresetKind::OnePerMonitor,
            3,
            &two_monitors(),
            &PresetOptions::default(),
        )
        .unwrap_err();
        assert_eq!(
            err,
            LayoutError::NotEnoughMonitors {
                preset: PresetKind::OnePerMonitor,
                needed: 3,
                available: 2
            }
        );
        let ok = preset(
            PresetKind::OnePerMonitor,
            2,
            &two_monitors(),
            &PresetOptions::default(),
        )
        .unwrap();
        assert_eq!(ok.viewport_for(2).unwrap().monitor, 1);
        let r = ok.resolve(&two_monitors()).unwrap();
        assert_eq!(r[1].rect, PixelRect::new(1920, 0, 2560, 1440));
    }

    #[test]
    fn two_per_monitor_with_three_players_gives_last_player_a_whole_monitor() {
        let layout = preset(
            PresetKind::TwoPerMonitor,
            3,
            &two_monitors(),
            &PresetOptions::default(),
        )
        .unwrap();
        assert_eq!(layout.viewport_for(3).unwrap().rect, NormRect::FULL);
        assert_eq!(layout.viewport_for(3).unwrap().monitor, 1);
    }

    #[test]
    fn validate_catches_overlap_and_missing_player() {
        let bad = Layout {
            name: "bad".into(),
            player_count: 2,
            viewports: vec![
                Viewport {
                    player: 1,
                    monitor: 0,
                    rect: NormRect::new(0.0, 0.0, 0.6, 1.0),
                },
                Viewport {
                    player: 2,
                    monitor: 0,
                    rect: NormRect::new(0.5, 0.0, 0.5, 1.0),
                },
            ],
        };
        assert!(
            matches!(bad.validate(1), Err(LayoutError::Invalid(_, msg)) if msg.contains("overlap"))
        );
        let missing = Layout {
            name: "missing".into(),
            player_count: 2,
            viewports: vec![
                Viewport {
                    player: 1,
                    monitor: 0,
                    rect: NormRect::new(0.0, 0.0, 0.5, 1.0),
                },
                Viewport {
                    player: 1,
                    monitor: 0,
                    rect: NormRect::new(0.5, 0.0, 0.5, 1.0),
                },
            ],
        };
        assert!(
            matches!(missing.validate(1), Err(LayoutError::Invalid(_, msg)) if msg.contains("player 1 appears 2"))
        );
    }

    #[test]
    fn toml_round_trip_and_parse_aliases() {
        let layout = preset(
            PresetKind::Grid,
            4,
            &one_monitor(),
            &PresetOptions::default(),
        )
        .unwrap();
        let text = layout.to_toml_string().unwrap();
        let back = Layout::from_toml_str(&text).unwrap();
        assert_eq!(back, layout);
        assert_eq!(PresetKind::parse("Big_Left").unwrap(), PresetKind::BigLeft);
        assert_eq!(PresetKind::parse("lr").unwrap(), PresetKind::LeftRight);
        assert!(PresetKind::parse("diagonal").is_err());
    }

    #[test]
    fn swap_players_exchanges_rects() {
        let mut layout = preset(
            PresetKind::LeftRight,
            2,
            &one_monitor(),
            &PresetOptions::default(),
        )
        .unwrap();
        layout.swap_players(1, 2);
        assert_eq!(layout.viewport_for(1).unwrap().rect.x, 0.5);
        layout.validate(1).unwrap();
    }

    #[test]
    fn ascii_preview_shows_each_player() {
        let layout = preset(
            PresetKind::BigTop,
            3,
            &one_monitor(),
            &PresetOptions::default(),
        )
        .unwrap();
        let art = ascii_preview(&layout, &one_monitor());
        assert!(art.contains('1') && art.contains('2') && art.contains('3'));
        assert!(!art.contains('4'));
    }

    #[test]
    fn available_presets_depend_on_monitors() {
        assert!(!available_presets(2, 1).contains(&PresetKind::OnePerMonitor));
        assert!(available_presets(2, 2).contains(&PresetKind::OnePerMonitor));
        assert!(available_presets(3, 1).contains(&PresetKind::BigLeft));
        assert!(!available_presets(2, 1).contains(&PresetKind::BigLeft));
    }
}
