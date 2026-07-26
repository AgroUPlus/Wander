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
    /// A glowing solar eclipse with dynamic coronal flare eruptions.
    Eclipse,
    /// Hypnotic 8-fold symmetrical fractal mandala rendered in high-res Braille sub-cells.
    Kaleidoscope,
    /// High-density gaseous cosmic cloud with Braille stardust spiral arms.
    Nebula,
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
        VizMode::Eclipse,
        VizMode::Kaleidoscope,
        VizMode::Nebula,
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
            VizMode::Eclipse => "eclipse",
            VizMode::Kaleidoscope => "kaleidoscope",
            VizMode::Nebula => "nebula",
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
        | VizMode::Kaleidoscope
        | VizMode::Nebula => width.max(1) as usize,

        VizMode::Bloom | VizMode::Vortex | VizMode::Eclipse => (width / 4).clamp(4, 32) as usize,
    }
}

/// Particle in cosmic nebula spiral galaxy mode.
struct NebulaParticle {
    angle: f32,
    radius: f32,
    speed: f32,
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
    /// Cosmic particles for nebula mode.
    nebula_particles: Vec<NebulaParticle>,
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
            nebula_particles: Vec::new(),
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
            VizMode::Eclipse => self.eclipse(spectrum.bars(), area, theme),
            VizMode::Kaleidoscope => self.kaleidoscope(spectrum.bars(), area, theme),
            VizMode::Nebula => self.nebula(spectrum.bars(), area, theme),
            VizMode::Scope => scope(spectrum, area, theme),
            VizMode::Waterfall => waterfall(spectrum, area, theme),
        };

        frame.render_widget(Paragraph::new(lines), area);
    }
}

mod modes;
#[cfg(test)]
mod tests;

pub use modes::*;
