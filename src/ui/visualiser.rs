//! The spectrum panel.
//!
//! These are deliberately not bar charts. A row of bars is the most literal way
//! to draw an FFT and the least interesting to look at, so each mode here reads
//! the same band levels as something with its own behaviour — a burning bed, a
//! drifting ribbon, a bloom opening on the beat — and keeps state between frames
//! so the picture has motion of its own rather than only tracking the audio.

use std::collections::VecDeque;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use serde::{Deserialize, Deserializer, Serialize};

use super::widgets::gradient;
use crate::player::spectrum::Spectrum;
use crate::theme::Theme;

/// Shading steps, faintest first. Used wherever a cell has an intensity rather
/// than a height.
const SHADES: [char; 4] = ['░', '▒', '▓', '█'];
/// Below this a cell is left blank, so quiet passages breathe instead of
/// filling the pane with noise.
const FLOOR: f32 = 0.06;
/// Terminal cells are roughly twice as tall as they are wide; anything meant to
/// look round has to compensate.
const CELL_ASPECT: f32 = 2.0;

/// How the spectrum is drawn. Cycled with `V`, remembered between sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VizMode {
    /// A drifting ribbon of light, shaped by the spectrum.
    #[default]
    Aurora,
    /// A bed of embers fed from below by the music's energy.
    Ember,
    /// Petals opening out of the centre, pushed by onsets.
    Bloom,
    /// A spinning supernova galaxy of logarithmic spiral arms and cosmic energy.
    Vortex,
    /// Holographic prismatic wave fronts intersecting and shimmering on the beat.
    Prism,
    /// A 3D warp-speed starfield tunnel accelerating on every beat.
    Hyperdrive,
    /// Cascading cyber digital rain with transient audio glitch pulses.
    Matrix,
    /// A glowing solar eclipse with dynamic coronal flare eruptions.
    Eclipse,
    /// The raw waveform, drawn with braille for sub-cell resolution.
    Scope,
    /// A scrolling spectrogram: time down the pane, colour by level.
    Waterfall,
}

impl VizMode {
    pub const ALL: [VizMode; 10] = [
        VizMode::Aurora,
        VizMode::Ember,
        VizMode::Bloom,
        VizMode::Vortex,
        VizMode::Prism,
        VizMode::Hyperdrive,
        VizMode::Matrix,
        VizMode::Eclipse,
        VizMode::Scope,
        VizMode::Waterfall,
    ];

    pub fn next(self) -> Self {
        let index = Self::ALL.iter().position(|mode| *mode == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    /// Matches the serialised name, so one is always readable as the other.
    pub fn label(self) -> &'static str {
        match self {
            VizMode::Aurora => "aurora",
            VizMode::Ember => "ember",
            VizMode::Bloom => "bloom",
            VizMode::Vortex => "vortex",
            VizMode::Prism => "prism",
            VizMode::Hyperdrive => "hyperdrive",
            VizMode::Matrix => "matrix",
            VizMode::Eclipse => "eclipse",
            VizMode::Scope => "scope",
            VizMode::Waterfall => "waterfall",
        }
    }
}

/// Read a saved mode without letting a name we no longer have discard the whole
/// session state — modes come and go, the queue should not go with them.
pub fn lenient_mode<'de, D: Deserializer<'de>>(deserializer: D) -> Result<VizMode, D::Error> {
    let raw = String::deserialize(deserializer)?;
    Ok(VizMode::ALL
        .into_iter()
        .find(|mode| mode.label() == raw)
        .unwrap_or_default())
}

/// How many bands a mode wants across `width` columns.
fn band_count(mode: VizMode, width: u16) -> usize {
    match mode {
        // Per-column detail.
        VizMode::Waterfall
        | VizMode::Scope
        | VizMode::Ember
        | VizMode::Aurora
        | VizMode::Prism
        | VizMode::Matrix => width.max(1) as usize,

        VizMode::Bloom | VizMode::Vortex | VizMode::Hyperdrive | VizMode::Eclipse => {
            (width / 4).clamp(4, 32) as usize
        }
    }
}

/// Star particle in 3D space for hyperdrive mode.
struct Star {
    x: f32,
    y: f32,
    z: f32,
}

/// Active falling stream in matrix mode.
struct MatrixDrop {
    col: usize,
    y: f32,
    speed: f32,
    length: f32,
}

/// Frame-to-frame state the effects need: a fire's heat, a ribbon's history, a
/// slow phase to drift by, and an onset detector to react to.
pub struct Visualiser {
    /// Ember heat field, row-major, `heat_size` shaped.
    heat: Vec<f32>,
    heat_size: (u16, u16),
    /// Aurora's recent shapes, newest first, so the ribbon trails itself.
    trail: VecDeque<Vec<f32>>,
    /// Slow drift, so nothing is ever perfectly still.
    phase: f32,
    /// Rings thrown out by the bloom, oldest first.
    ripples: Vec<Ripple>,
    /// 3D stars for hyperdrive mode.
    stars: Vec<Star>,
    /// Falling rain drops for matrix mode.
    matrix_drops: Vec<MatrixDrop>,
    /// Smoothed loudness, and the pulse an onset leaves behind.
    energy: f32,
    pulse: f32,
    /// xorshift state: cheap jitter without pulling in a dependency.
    seed: u32,
}

impl Default for Visualiser {
    fn default() -> Self {
        Self {
            heat: Vec::new(),
            heat_size: (0, 0),
            trail: VecDeque::new(),
            ripples: Vec::new(),
            stars: Vec::new(),
            matrix_drops: Vec::new(),
            phase: 0.0,
            energy: 0.0,
            pulse: 0.0,
            seed: 0x2545_f491,
        }
    }
}

/// One expanding ring of the bloom.
struct Ripple {
    /// Fraction of the way to the corner of the pane.
    radius: f32,
    strength: f32,
    /// How many lobes its edge has.
    petals: f32,
    /// Rotation it was born with, so rings do not line up.
    spin: f32,
}

/// Where a frame's energy sits in the spectrum, in `[0, 1]`.
///
/// Low for a bass hit, high for a cymbal — used to give the two different
/// shapes rather than treating every onset the same.
fn centroid(bars: &[f32]) -> f32 {
    let total: f32 = bars.iter().sum();
    if total <= f32::EPSILON || bars.len() < 2 {
        return 0.0;
    }
    let weighted: f32 = bars
        .iter()
        .enumerate()
        .map(|(index, level)| index as f32 * level)
        .sum();
    (weighted / total) / (bars.len() - 1) as f32
}

impl Visualiser {
    /// Uniform noise in `[0, 1)`.
    fn noise(&mut self) -> f32 {
        // xorshift32, which is plenty for sparks and jitter.
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        (self.seed >> 8) as f32 / (1 << 24) as f32
    }

    /// Advance the shared clock, and watch for onsets.
    ///
    /// Spectral flux — how much louder this frame is than the last — is what
    /// makes the effects hit on the beat instead of merely following the volume.
    fn advance(&mut self, bars: &[f32]) {
        self.phase += 0.06;
        if self.phase > std::f32::consts::TAU * 64.0 {
            self.phase -= std::f32::consts::TAU * 64.0;
        }

        let level = if bars.is_empty() {
            0.0
        } else {
            bars.iter().sum::<f32>() / bars.len() as f32
        };
        let flux = (level - self.energy).max(0.0);
        self.energy += (level - self.energy) * 0.25;
        // Rises on a transient, falls away over about half a second.
        self.pulse = (self.pulse * 0.88).max((flux * 6.0).min(1.0));
    }

    pub fn draw(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        spectrum: &mut Spectrum,
        mode: VizMode,
        theme: &Theme,
    ) {
        if area.width < 2 || area.height == 0 {
            return;
        }
        spectrum.resize(band_count(mode, area.width));
        self.advance(spectrum.bars());

        let lines = match mode {
            VizMode::Aurora => self.aurora(spectrum.bars(), area, theme),
            VizMode::Ember => self.ember(spectrum.bars(), area, theme),
            VizMode::Bloom => self.bloom(spectrum.bars(), area, theme),
            VizMode::Vortex => self.vortex(spectrum.bars(), area, theme),
            VizMode::Prism => self.prism(spectrum.bars(), area, theme),
            VizMode::Hyperdrive => self.hyperdrive(spectrum.bars(), area, theme),
            VizMode::Matrix => self.matrix(spectrum.bars(), area, theme),
            VizMode::Eclipse => self.eclipse(spectrum.bars(), area, theme),
            VizMode::Scope => scope(spectrum, area, theme),
            VizMode::Waterfall => waterfall(spectrum, area, theme),
        };

        frame.render_widget(Paragraph::new(lines), area);
    }

    /// A ribbon of light whose height follows the spectrum, trailing the shapes
    /// it held a moment ago so movement leaves a wake.
    ///
    /// The band levels are smoothed across neighbours and lifted by a slow
    /// travelling wave, which is what turns a jagged spectrum into something
    /// that flows.
    fn aurora<'a>(&mut self, bars: &[f32], area: Rect, theme: &Theme) -> Vec<Line<'a>> {
        let cols = area.width as usize;
        let rows = area.height as usize;

        // Blur the spectrum along its length; a ribbon should not have teeth.
        let mut shape = vec![0.0f32; cols];
        for (col, slot) in shape.iter_mut().enumerate() {
            let mut sum = 0.0;
            let mut weight = 0.0;
            for offset in -2i32..=2 {
                let index = col as i32 + offset;
                if index >= 0 && (index as usize) < bars.len() {
                    let w = 1.0 / (1.0 + offset.abs() as f32);
                    sum += bars[index as usize] * w;
                    weight += w;
                }
            }
            let level = if weight > 0.0 { sum / weight } else { 0.0 };
            // A travelling wave, so the ribbon drifts even on a steady note.
            let drift = ((col as f32 * 0.18) + self.phase).sin() * 0.12;
            *slot = (level * 0.85 + 0.10 + drift).clamp(0.0, 1.0);
        }

        // Keep enough history to trail behind the current shape. Shapes from
        // before a resize are a different width, so they go rather than being
        // indexed past their end.
        self.trail.retain(|past| past.len() == cols);
        self.trail.push_front(shape);
        self.trail.truncate(rows.max(2));

        let mut lines = Vec::with_capacity(rows);
        for row in 0..rows {
            let spans = (0..cols)
                .map(|col| {
                    // Where the ribbon sits on this column, and how thick.
                    let level = self.trail[0][col];
                    let centre = (1.0 - level) * (rows - 1) as f32;
                    let distance = (row as f32 - centre).abs();
                    let thickness = 0.6 + level * 1.8;

                    // Older shapes leave a fainter echo behind the ribbon.
                    let mut glow = (1.0 - distance / thickness).max(0.0);
                    for (age, past) in self.trail.iter().enumerate().skip(1) {
                        let faded = 0.55f32.powi(age as i32);
                        let centre = (1.0 - past[col]) * (rows - 1) as f32;
                        let echo = (1.0 - (row as f32 - centre).abs() / thickness).max(0.0);
                        glow = glow.max(echo * faded);
                    }

                    cell(glow, theme)
                })
                .collect::<Vec<Span>>();
            lines.push(Line::from(spans));
        }
        lines
    }

    /// A bed of embers: each column is fed heat in proportion to its band, then
    /// the heat rises, spreads sideways and cools.
    ///
    /// Nothing here is drawn from the spectrum directly — the audio only stokes
    /// the fire, which is why it keeps moving through a sustained note and dies
    /// down slowly rather than snapping to silence.
    fn ember<'a>(&mut self, bars: &[f32], area: Rect, theme: &Theme) -> Vec<Line<'a>> {
        let cols = area.width as usize;
        let rows = area.height as usize;

        if self.heat_size != (area.width, area.height) {
            self.heat = vec![0.0; cols * rows];
            self.heat_size = (area.width, area.height);
        }

        // Rise: every row takes from the one below it, spread across three
        // columns so flames lean rather than climb in straight lines.
        for row in 0..rows.saturating_sub(1) {
            for col in 0..cols {
                let below = (row + 1) * cols;
                let left = self.heat[below + col.saturating_sub(1)];
                let here = self.heat[below + col];
                let right = self.heat[below + (col + 1).min(cols - 1)];
                let jitter = 0.94 + self.noise() * 0.12;
                // Cooling per row: low enough that flames reach the top of a
                // short pane, high enough that they still taper.
                self.heat[row * cols + col] = ((left + here * 2.0 + right) / 4.0) * jitter * 0.94;
            }
        }

        // Stoke the bottom row from the bands, with a gust on each onset.
        let gust = 1.0 + self.pulse * 0.5;
        for col in 0..cols {
            let level = bars.get(col).copied().unwrap_or(0.0);
            let spark = 0.85 + self.noise() * 0.35;
            let bottom = (rows - 1) * cols + col;
            // Lifted off the floor: a band at half level should still burn
            // brightly rather than smoulder.
            self.heat[bottom] = (level.sqrt() * spark * gust).clamp(0.0, 1.0);
        }

        (0..rows)
            .map(|row| {
                let spans = (0..cols)
                    .map(|col| cell(self.heat[row * cols + col], theme))
                    .collect::<Vec<Span>>();
                Line::from(spans)
            })
            .collect()
    }

    /// Rings of light thrown out from the centre, one per onset, opening into
    /// petals as they travel.
    ///
    /// Reading a band per pixel was tried first and came out as speckle: the
    /// spectrum is too jagged to survive being sampled that finely. Ripples are
    /// *events* instead — the music decides when one is born and how bright, and
    /// after that the ring has a life of its own.
    fn bloom<'a>(&mut self, bars: &[f32], area: Rect, theme: &Theme) -> Vec<Line<'a>> {
        // A new ring on each onset, but never so often that they blur together.
        if self.pulse > 0.35 && self.ripples.last().is_none_or(|r| r.radius > 0.28) {
            let brightness = 0.55 + self.pulse * 0.45;
            // Where the energy sits decides how many petals the ring has, so a
            // bass hit and a cymbal do not look the same.
            let centroid = centroid(bars);
            self.ripples.push(Ripple {
                radius: 0.0,
                strength: brightness,
                petals: 3.0 + (centroid * 6.0).round(),
                spin: self.phase,
            });
        }
        for ripple in &mut self.ripples {
            ripple.radius += 0.045;
            ripple.strength *= 0.94;
        }
        self.ripples
            .retain(|ripple| ripple.strength > 0.08 && ripple.radius < 1.3);
        // A bounded list: a long loud passage must not grow it without limit.
        if self.ripples.len() > 6 {
            self.ripples.drain(..self.ripples.len() - 6);
        }

        let (cx, cy) = (
            (area.width as f32 - 1.0) / 2.0,
            (area.height as f32 - 1.0) / 2.0,
        );
        // Terminal cells are about twice as tall as they are wide, so vertical
        // distance counts double for the rings to come out round.
        let reach = (cx * cx + (cy * CELL_ASPECT).powi(2)).sqrt().max(1.0);
        // The heart of the bloom glows with the low end, so there is always
        // something at the centre for the rings to leave from.
        let lows = bars.len().div_ceil(3).max(1);
        let core = bars[..lows.min(bars.len())].iter().sum::<f32>() / lows as f32;

        (0..area.height)
            .map(|row| {
                let spans = (0..area.width)
                    .map(|col| {
                        let dx = col as f32 - cx;
                        let dy = (row as f32 - cy) * CELL_ASPECT;
                        let radius = (dx * dx + dy * dy).sqrt() / reach;
                        let angle = dy.atan2(dx);

                        let mut glow = (core - radius * 1.3).max(0.0);
                        for ripple in &self.ripples {
                            // Petals: the ring's edge breathes in and out around
                            // its circumference, and turns as it expands.
                            let lobe = 1.0
                                + 0.22
                                    * (angle * ripple.petals + ripple.spin + ripple.radius * 3.0)
                                        .sin();
                            let edge = (radius - ripple.radius * lobe).abs();
                            // A soft shell rather than a hard circle.
                            let falloff = (1.0 - edge / 0.16).max(0.0);
                            glow = glow.max(ripple.strength * falloff * falloff);
                        }
                        cell(glow, theme)
                    })
                    .collect::<Vec<Span>>();
                Line::from(spans)
            })
            .collect()
    }

    /// A swirling supernova galaxy: spinning spiral arms of light,
    /// a pulsing core fed by bass, and expanding shockwaves on each beat.
    fn vortex<'a>(&mut self, bars: &[f32], area: Rect, theme: &Theme) -> Vec<Line<'a>> {
        let (cx, cy) = (
            (area.width as f32 - 1.0) / 2.0,
            (area.height as f32 - 1.0) / 2.0,
        );
        let reach = (cx * cx + (cy * CELL_ASPECT).powi(2)).sqrt().max(1.0);

        let lows = bars.len().div_ceil(3).max(1);
        let core_energy = if bars.is_empty() {
            0.0
        } else {
            bars[..lows.min(bars.len())].iter().sum::<f32>() / lows as f32
        };
        let centroid_val = centroid(bars);
        let arm_count = 3.0 + (centroid_val * 4.0).round();

        let pulse_boost = 1.0 + self.pulse * 0.9;
        let core_radius = 0.12 + core_energy * 0.28 * pulse_boost;

        (0..area.height)
            .map(|row| {
                let spans = (0..area.width)
                    .map(|col| {
                        let dx = col as f32 - cx;
                        let dy = (row as f32 - cy) * CELL_ASPECT;
                        let radius = (dx * dx + dy * dy).sqrt() / reach;
                        let angle = dy.atan2(dx);

                        // Pulsing core
                        let mut intensity = (1.0 - radius / core_radius).max(0.0) * core_energy * 1.6;

                        // Logarithmic spiral arms spinning with phase
                        let spiral_angle = angle - 3.2 * (radius + 0.05).ln() + self.phase * 0.6;
                        let arm_pattern = ((spiral_angle * arm_count).sin() * 0.5 + 0.5).powf(2.5);

                        let band_idx = ((radius.clamp(0.0, 1.0) * (bars.len() as f32 - 1.0)) as usize)
                            .min(bars.len().saturating_sub(1));
                        let bar_level = bars.get(band_idx).copied().unwrap_or(0.0);

                        let spiral_glow = arm_pattern * (0.35 + bar_level * 0.95) * (1.0 - radius * 0.75).max(0.0);
                        intensity = intensity.max(spiral_glow * pulse_boost);

                        cell(intensity, theme)
                    })
                    .collect::<Vec<Span>>();
                Line::from(spans)
            })
            .collect()
    }

    /// A holographic prismatic wave interference field: intersecting energy
    /// fronts that twist and shimmer according to frequency harmonics.
    fn prism<'a>(&mut self, bars: &[f32], area: Rect, theme: &Theme) -> Vec<Line<'a>> {
        let (cx, cy) = (
            (area.width as f32 - 1.0) / 2.0,
            (area.height as f32 - 1.0) / 2.0,
        );
        let reach = (cx * cx + (cy * CELL_ASPECT).powi(2)).sqrt().max(1.0);
        let centroid_val = centroid(bars);
        let high_energy = if bars.is_empty() {
            0.0
        } else {
            bars[bars.len() * 2 / 3..].iter().sum::<f32>() / (bars.len() / 3).max(1) as f32
        };

        (0..area.height)
            .map(|row| {
                let spans = (0..area.width)
                    .map(|col| {
                        let nx = (col as f32 - cx) / reach;
                        let ny = (row as f32 - cy) * CELL_ASPECT / reach;
                        let r = (nx * nx + ny * ny).sqrt();

                        let wave1 = (nx * 6.0 + self.phase * 1.2).sin();
                        let wave2 = (ny * 6.0 - self.phase * 0.9).cos();
                        let wave3 = ((nx + ny) * 4.0 + self.phase * 1.5 + centroid_val * 3.0).sin();

                        let interference = (wave1 + wave2 + wave3) / 3.0;
                        let band_idx = ((r.clamp(0.0, 1.0) * (bars.len() as f32 - 1.0)) as usize)
                            .min(bars.len().saturating_sub(1));
                        let bar_level = bars.get(band_idx).copied().unwrap_or(0.0);

                        let intensity = ((interference * 0.5 + 0.5) * (0.3 + bar_level * 0.8)
                            + (self.pulse * 0.4 * (1.0 - r).max(0.0))
                            + high_energy * 0.2)
                            .clamp(0.0, 1.0);

                        cell(intensity, theme)
                    })
                    .collect::<Vec<Span>>();
                Line::from(spans)
            })
            .collect()
    }

    /// A 3D warp-speed starfield tunnel: stars spawn at the center and surge
    /// outward toward the viewer, leaving light trails that stretch on beat hits.
    fn hyperdrive<'a>(&mut self, bars: &[f32], area: Rect, theme: &Theme) -> Vec<Line<'a>> {
        let cols = area.width as usize;
        let rows = area.height as usize;
        let (cx, cy) = ((cols as f32 - 1.0) / 2.0, (rows as f32 - 1.0) / 2.0);

        let target_stars = (cols * rows / 8).clamp(30, 200);
        while self.stars.len() < target_stars {
            let angle = self.noise() * std::f32::consts::TAU;
            let dist = self.noise().sqrt() * 0.9 + 0.1;
            let z = self.noise() * 0.9 + 0.1;
            self.stars.push(Star {
                x: angle.cos() * dist,
                y: angle.sin() * dist,
                z,
            });
        }

        let speed = 0.02 + self.energy * 0.05 + self.pulse * 0.12;
        let mut respawns = Vec::new();
        for (i, star) in self.stars.iter_mut().enumerate() {
            star.z -= speed;
            if star.z <= 0.04 {
                respawns.push(i);
            }
        }
        for idx in respawns {
            let angle = self.noise() * std::f32::consts::TAU;
            let dist = self.noise().sqrt() * 0.9 + 0.1;
            self.stars[idx].x = angle.cos() * dist;
            self.stars[idx].y = angle.sin() * dist;
            self.stars[idx].z = 1.0;
        }

        let mut grid = vec![0.0f32; cols * rows];
        let centroid_val = centroid(bars);
        let spin_angle = self.phase * 0.4 + centroid_val * 2.0;

        for star in &self.stars {
            let z = star.z.max(0.04);
            let rx = star.x * spin_angle.cos() - star.y * spin_angle.sin();
            let ry = star.x * spin_angle.sin() + star.y * spin_angle.cos();

            let px = cx + (rx / z) * cx * 0.85;
            let py = cy + (ry / z) * cy * (0.85 / CELL_ASPECT);

            if px >= 0.0 && px < cols as f32 && py >= 0.0 && py < rows as f32 {
                let col = px as usize;
                let row = py as usize;
                let brightness = ((1.0 - z) * (0.5 + self.pulse * 0.8)).clamp(0.0, 1.0);

                let idx = row * cols + col;
                grid[idx] = grid[idx].max(brightness);

                let trail_steps = (self.pulse * 4.0 + 1.0) as usize;
                for step in 1..=trail_steps {
                    let prev_z = z + step as f32 * 0.03;
                    if prev_z <= 1.0 {
                        let t_px = cx + (rx / prev_z) * cx * 0.85;
                        let t_py = cy + (ry / prev_z) * cy * (0.85 / CELL_ASPECT);
                        if t_px >= 0.0 && t_px < cols as f32 && t_py >= 0.0 && t_py < rows as f32 {
                            let t_col = t_px as usize;
                            let t_row = t_py as usize;
                            let t_idx = t_row * cols + t_col;
                            let faded = brightness * (1.0 - step as f32 / (trail_steps as f32 + 1.0));
                            grid[t_idx] = grid[t_idx].max(faded);
                        }
                    }
                }
            }
        }

        (0..rows)
            .map(|row| {
                let spans = (0..cols)
                    .map(|col| cell(grid[row * cols + col], theme))
                    .collect::<Vec<Span>>();
                Line::from(spans)
            })
            .collect()
    }

    /// Cyberpunk digital rain: falling data drops whose frequency and speed
    /// react to audio bands, with transient beat glitches across the matrix.
    fn matrix<'a>(&mut self, bars: &[f32], area: Rect, theme: &Theme) -> Vec<Line<'a>> {
        let cols = area.width as usize;
        let rows = area.height as usize;

        let mut spawns = Vec::new();
        for (col, level) in bars.iter().enumerate().take(cols) {
            if *level > 0.15 && self.noise() < *level * 0.35 {
                spawns.push((col, *level));
            }
        }
        for (col, level) in spawns {
            self.matrix_drops.push(MatrixDrop {
                col,
                y: 0.0,
                speed: 0.4 + level * 0.8 + self.pulse * 0.6,
                length: 4.0 + level * 12.0,
            });
        }

        for drop in &mut self.matrix_drops {
            drop.y += drop.speed;
        }
        self.matrix_drops.retain(|drop| drop.y - drop.length < rows as f32);
        if self.matrix_drops.len() > cols * 2 {
            self.matrix_drops.drain(..cols);
        }

        let mut grid = vec![0.0f32; cols * rows];
        for drop in &self.matrix_drops {
            let col = drop.col;
            if col >= cols {
                continue;
            }
            let head = drop.y as i32;
            let tail = (drop.y - drop.length) as i32;
            for r in tail.max(0)..=head.min(rows as i32 - 1) {
                let r_usize = r as usize;
                let dist_from_head = (head - r) as f32;
                let brightness = (1.0 - dist_from_head / drop.length).max(0.0);

                let idx = r_usize * cols + col;
                grid[idx] = grid[idx].max(brightness);
            }
        }

        if self.pulse > 0.4 {
            let glitch_row = ((self.noise() * rows as f32) as usize).min(rows.saturating_sub(1));
            for col in 0..cols {
                grid[glitch_row * cols + col] = (grid[glitch_row * cols + col] + self.pulse * 0.7).min(1.0);
            }
        }

        (0..rows)
            .map(|row| {
                let spans = (0..cols)
                    .map(|col| cell(grid[row * cols + col], theme))
                    .collect::<Vec<Span>>();
                Line::from(spans)
            })
            .collect()
    }

    /// A celestial solar eclipse: a central dark moon silhouette surrounded by a
    /// brilliant glowing solar corona that flares and erupts to audio frequencies.
    fn eclipse<'a>(&mut self, bars: &[f32], area: Rect, theme: &Theme) -> Vec<Line<'a>> {
        let (cx, cy) = (
            (area.width as f32 - 1.0) / 2.0,
            (area.height as f32 - 1.0) / 2.0,
        );
        let reach = (cx * cx + (cy * CELL_ASPECT).powi(2)).sqrt().max(1.0);
        let moon_radius = 0.28;

        let lows = bars.len().div_ceil(3).max(1);
        let core_energy = if bars.is_empty() {
            0.0
        } else {
            bars[..lows.min(bars.len())].iter().sum::<f32>() / lows as f32
        };

        let corona_expand = moon_radius + 0.05 + core_energy * 0.22 + self.pulse * 0.25;

        (0..area.height)
            .map(|row| {
                let spans = (0..area.width)
                    .map(|col| {
                        let dx = col as f32 - cx;
                        let dy = (row as f32 - cy) * CELL_ASPECT;
                        let r = (dx * dx + dy * dy).sqrt() / reach;
                        let angle = dy.atan2(dx);

                        if r <= moon_radius {
                            Span::raw(" ")
                        } else {
                            let norm_angle = (angle + std::f32::consts::PI) / std::f32::consts::TAU;
                            let band_idx = ((norm_angle * (bars.len() as f32 - 1.0)) as usize)
                                .min(bars.len().saturating_sub(1));
                            let bar_level = bars.get(band_idx).copied().unwrap_or(0.0);

                            let flare = (angle * 7.0 + self.phase * 1.4).sin() * 0.06
                                + (angle * 13.0 - self.phase * 2.1).cos() * 0.04;
                            let outer_bound = corona_expand + flare + bar_level * 0.35;

                            let edge_dist = r - moon_radius;
                            let max_dist = outer_bound - moon_radius;

                            let intensity = if r <= outer_bound && max_dist > 0.0 {
                                (1.0 - edge_dist / max_dist).powf(1.4) * (0.4 + bar_level * 0.8 + self.pulse * 0.5)
                            } else {
                                0.0
                            };

                            cell(intensity, theme)
                        }
                    })
                    .collect::<Vec<Span>>();
                Line::from(spans)
            })
            .collect()
    }
}

/// One cell of an intensity field: a shade glyph coloured along the theme's
/// visualiser gradient. Shared by every mode so they all respond to a theme
/// change the same way.
fn cell<'a>(intensity: f32, theme: &Theme) -> Span<'a> {
    let intensity = intensity.clamp(0.0, 1.0);
    if intensity < FLOOR {
        return Span::raw(" ");
    }
    // Four shades is a coarse ramp; without this most of a picture sits in the
    // faintest one and the effect reads as grey noise.
    let intensity = intensity.powf(0.65);
    let step = ((intensity * SHADES.len() as f32).ceil() as usize).clamp(1, SHADES.len()) - 1;
    Span::styled(
        SHADES[step].to_string(),
        Style::default().fg(shade(intensity, theme)),
    )
}

/// Colour at a given intensity, along the theme's low → high gradient.
fn shade(intensity: f32, theme: &Theme) -> Color {
    gradient(
        theme.viz_low.0,
        theme.viz_high.0,
        intensity.clamp(0.0, 1.0) as f64,
    )
}

/// Braille dot bits, indexed by `[column][row]` within a 2x4 cell.
const BRAILLE: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

/// The raw waveform as a continuous braille trace.
fn scope<'a>(spectrum: &Spectrum, area: Rect, theme: &Theme) -> Vec<Line<'a>> {
    let cols = area.width as usize;
    let rows = area.height as usize;
    // Two dots per cell across, four down.
    let dot_cols = cols * 2;
    let dot_rows = rows * 4;

    let samples: Vec<f32> = spectrum.waveform(dot_cols).collect();
    if samples.is_empty() {
        return Vec::new();
    }

    let to_dot_row = |value: f32| -> usize {
        let centred = (1.0 - value.clamp(-1.0, 1.0)) / 2.0;
        ((centred * (dot_rows - 1) as f32).round() as usize).min(dot_rows - 1)
    };

    let mut grid = vec![0u8; cols * rows];
    let mut previous: Option<usize> = None;
    for dot_col in 0..dot_cols {
        let sample = samples[dot_col * samples.len() / dot_cols];
        let dot_row = to_dot_row(sample);
        // Join successive samples so a fast waveform reads as a line rather
        // than a scatter of unconnected dots.
        let from = previous.unwrap_or(dot_row).min(dot_row);
        let to = previous.unwrap_or(dot_row).max(dot_row);
        previous = Some(dot_row);

        for row in from..=to {
            let cell = (row / 4) * cols + dot_col / 2;
            grid[cell] |= BRAILLE[dot_col % 2][row % 4];
        }
    }

    (0..rows)
        .map(|row| {
            let spans: Vec<Span> = (0..cols)
                .map(|col| {
                    let bits = grid[row * cols + col];
                    // Distance from the centre row drives the colour, so the
                    // trace brightens as it swings.
                    let from_centre =
                        (row as f32 - (rows as f32 - 1.0) / 2.0).abs() / (rows as f32 / 2.0);
                    let glyph = char::from_u32(0x2800 + bits as u32).unwrap_or(' ');
                    Span::styled(
                        glyph.to_string(),
                        Style::default().fg(shade(from_centre, theme)),
                    )
                })
                .collect();
            Line::from(spans)
        })
        .collect()
}

/// A spectrogram scrolling downward: the newest frame on top.
fn waterfall<'a>(spectrum: &Spectrum, area: Rect, theme: &Theme) -> Vec<Line<'a>> {
    let cols = area.width as usize;
    let rows = area.height as usize;

    spectrum
        .history()
        .take(rows)
        .map(|frame| {
            let spans: Vec<Span> = (0..cols)
                .map(|col| cell(frame.get(col).copied().unwrap_or(0.0), theme))
                .collect();
            Line::from(spans)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn theme() -> Theme {
        Theme::default()
    }

    #[test]
    fn cycling_modes_returns_to_the_start() {
        let mut mode = VizMode::default();
        let mut seen = vec![mode];
        for _ in 1..VizMode::ALL.len() {
            mode = mode.next();
            assert!(!seen.contains(&mode), "{mode:?} repeated early");
            seen.push(mode);
        }
        assert_eq!(mode.next(), VizMode::default(), "should wrap");
    }

    #[test]
    fn labels_match_the_serialised_names() {
        for mode in VizMode::ALL {
            let json = serde_json::to_string(&mode).expect("serialise");
            assert_eq!(json, format!("\"{}\"", mode.label()));
        }
    }

    /// A mode that no longer exists must not take the saved queue down with it.
    #[test]
    fn a_retired_mode_name_falls_back_to_the_default() {
        #[derive(serde::Deserialize)]
        struct Saved {
            #[serde(deserialize_with = "lenient_mode")]
            viz_mode: VizMode,
            other: u32,
        }
        let saved: Saved =
            serde_json::from_str(r#"{"viz_mode":"peaks","other":7}"#).expect("must still load");
        assert_eq!(saved.viz_mode, VizMode::default());
        assert_eq!(saved.other, 7, "the rest of the state survives");

        let kept: Saved =
            serde_json::from_str(r#"{"viz_mode":"ember","other":1}"#).expect("known mode");
        assert_eq!(kept.viz_mode, VizMode::Ember);
    }

    #[test]
    fn every_mode_draws_something_at_every_plausible_size() {
        for mode in VizMode::ALL {
            for (w, h) in [(7u16, 3u16), (40, 8), (120, 12), (3, 40)] {
                let (mut producer, mut spectrum) = spectrum_with_audio();
                let mut visualiser = Visualiser::default();
                let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test backend");
                // Several frames, as the app does: the first tells the spectrum
                // how many bands the pane wants, and the effects need a moment
                // to develop.
                for _ in 0..6 {
                    terminal
                        .draw(|frame| {
                            visualiser.draw(frame, frame.area(), &mut spectrum, mode, &theme());
                        })
                        .expect("draw must not fail");
                    push_tone(&mut producer);
                    spectrum.update();
                }

                let buffer = terminal.backend().buffer().clone();
                let drawn: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
                assert!(
                    drawn.chars().any(|c| c != ' '),
                    "{mode:?} drew nothing at {w}x{h}"
                );
            }
        }
    }

    #[test]
    fn every_mode_survives_a_pane_of_any_shape() {
        // Panes get this small mid-tween, and a panic there takes the app down.
        for mode in VizMode::ALL {
            for (w, h) in [(2u16, 1u16), (3, 1), (2, 2), (1, 1), (200, 1), (2, 60)] {
                let (_producer, mut spectrum) = spectrum_with_audio();
                let mut visualiser = Visualiser::default();
                let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test backend");
                for _ in 0..3 {
                    terminal
                        .draw(|frame| {
                            visualiser.draw(frame, frame.area(), &mut spectrum, mode, &theme());
                        })
                        .expect("draw must not fail");
                }
            }
        }
    }

    /// The effects keep their own state, so a resize must not read a field
    /// sized for the previous pane.
    #[test]
    fn effects_survive_being_resized_between_frames() {
        for mode in VizMode::ALL {
            let (mut producer, mut spectrum) = spectrum_with_audio();
            let mut visualiser = Visualiser::default();
            for (w, h) in [(40u16, 8u16), (12, 3), (80, 12), (5, 2), (60, 8)] {
                let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test backend");
                terminal
                    .draw(|frame| {
                        visualiser.draw(frame, frame.area(), &mut spectrum, mode, &theme());
                    })
                    .expect("draw must not fail");
                push_tone(&mut producer);
                spectrum.update();
            }
        }
    }

    /// Silence should settle, not freeze mid-frame.
    #[test]
    fn the_picture_dies_down_when_the_music_stops() {
        let (_producer, mut spectrum) = spectrum_with_audio();
        let mut visualiser = Visualiser::default();
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).expect("test backend");

        let mut ink = |visualiser: &mut Visualiser, spectrum: &mut Spectrum| {
            terminal
                .draw(|frame| {
                    visualiser.draw(frame, frame.area(), spectrum, VizMode::Ember, &theme());
                })
                .expect("draw");
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .filter(|cell| cell.symbol() != " ")
                .count()
        };

        let mut lit = 0;
        for _ in 0..8 {
            lit = ink(&mut visualiser, &mut spectrum);
        }
        assert!(lit > 0, "the fire should catch while audio plays");

        // No new audio from here on.
        let mut settled = lit;
        for _ in 0..80 {
            spectrum.update();
            settled = ink(&mut visualiser, &mut spectrum);
        }
        assert!(
            settled < lit / 2,
            "embers should die down in silence ({lit} -> {settled})"
        );
    }

    /// A spectrum with real audio behind it, plus the producer needed to keep
    /// feeding it.
    fn spectrum_with_audio() -> (rtrb::Producer<f32>, Spectrum) {
        let (mut producer, consumer) = rtrb::RingBuffer::<f32>::new(8192);
        push_tone(&mut producer);
        let mut spectrum = Spectrum::new(consumer, 48_000, 32);
        spectrum.update();
        (producer, spectrum)
    }

    /// A bass note and a treble note, so low and high bands both light up.
    fn push_tone(producer: &mut rtrb::Producer<f32>) {
        for i in 0..4096 {
            let t = i as f32 / 48_000.0;
            let _ = producer.push(
                0.6 * (std::f32::consts::TAU * 220.0 * t).sin()
                    + 0.3 * (std::f32::consts::TAU * 3000.0 * t).sin(),
            );
        }
    }
}
