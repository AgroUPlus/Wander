//! Deriving accent colours from cover art.
//!
//! The point is that the player visibly reacts to what is playing, without ever
//! becoming unreadable. Only the accent colours are taken from the artwork —
//! backgrounds and body text stay with the user's chosen preset — and everything
//! taken is pushed to a legible contrast against that preset's background first.

use image::imageops::FilterType;
use ratatui::style::Color;

use super::{Theme, ThemeColor};

/// Cover art is downscaled to this before any pixel is looked at. A few
/// thousand pixels describe the palette just as well as a million.
///
/// Extraction measures around 4.5 ms per cover in release over a real library
/// (see the `palette_swatches` diagnostic below), nearly all of it spent
/// decoding the full-size JPEG rather than looking at pixels. It runs on the
/// cover-loading task, so that cost never lands on the frame loop.
const SAMPLE: u32 = 64;
/// Hue resolution. Twenty-four bins is 15° each: fine enough to separate a red
/// from an orange, coarse enough that a gradient still lands in one bucket.
const HUE_BINS: usize = 24;
/// Pixels outside these bounds are background, print white, or shadow — they
/// describe the sleeve, not the artwork.
const MIN_LIGHTNESS: f32 = 0.12;
const MAX_LIGHTNESS: f32 = 0.92;
const MIN_SATURATION: f32 = 0.20;
/// Below this share of usable pixels the cover is effectively greyscale, and
/// inventing a colour for it would be worse than leaving the preset alone.
const MIN_COLOURED_SHARE: f32 = 0.05;
/// How far apart, in bins, the secondary hue must sit from the primary.
const SECONDARY_DISTANCE: usize = 3;
/// Accents are clamped into this saturation range: low enough not to glare,
/// high enough that a washed-out cover still reads as a colour.
const ACCENT_SATURATION: (f32, f32) = (0.45, 0.85);
/// Contrast an accent must reach against the background, per WCAG AA.
const MIN_CONTRAST: f32 = 4.5;
/// Assumed background luminance when the real one is unknown — `Color::Reset`
/// and indexed colours are whatever the terminal says they are. A dark terminal
/// is the overwhelmingly common case for a TUI.
const ASSUMED_BACKGROUND: f32 = 0.05;

/// The two accent colours taken from a cover.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub primary: (u8, u8, u8),
    pub secondary: (u8, u8, u8),
}

/// Pull a palette out of encoded image bytes.
///
/// Returns `None` for artwork with no usable colour — greyscale sleeves, very
/// dark photographs — so the caller can leave the preset untouched.
pub fn extract(bytes: &[u8]) -> Option<Palette> {
    let image = image::load_from_memory(bytes).ok()?;
    let image = image
        .resize_exact(SAMPLE, SAMPLE, FilterType::Triangle)
        .to_rgb8();

    let total = (SAMPLE * SAMPLE) as f32;
    // Per bin: accumulated weight, plus the members' saturation and lightness
    // kept for a median. A median resists a handful of blown-out pixels in a
    // way a mean does not.
    let mut weights = [0.0f32; HUE_BINS];
    let mut members: Vec<Vec<(f32, f32)>> = vec![Vec::new(); HUE_BINS];
    let mut coloured = 0u32;

    for pixel in image.pixels() {
        let (h, s, l) = rgb_to_hsl(pixel.0[0], pixel.0[1], pixel.0[2]);
        if s < MIN_SATURATION || !(MIN_LIGHTNESS..=MAX_LIGHTNESS).contains(&l) {
            continue;
        }
        coloured += 1;
        let bin = ((h / 360.0) * HUE_BINS as f32) as usize % HUE_BINS;
        // Weighting by saturation lets a small vivid element beat a large muddy
        // one, which is usually what a person would call the cover's colour.
        weights[bin] += s;
        members[bin].push((s, l));
    }

    if (coloured as f32) / total < MIN_COLOURED_SHARE {
        return None;
    }

    let primary_bin = heaviest(&weights, |_| true)?;
    let secondary_bin = heaviest(&weights, |bin| {
        bin_distance(bin, primary_bin) >= SECONDARY_DISTANCE
    });

    let primary = bin_colour(primary_bin, &members[primary_bin]);
    let secondary = match secondary_bin {
        Some(bin) if weights[bin] > 0.0 => bin_colour(bin, &members[bin]),
        // A monochromatic cover still needs two ends for the visualiser
        // gradient, so rotate the one hue we have.
        _ => {
            let (h, s, l) = rgb_to_hsl(primary.0, primary.1, primary.2);
            hsl_to_rgb((h + 30.0) % 360.0, s, l)
        }
    };

    Some(Palette { primary, secondary })
}

/// Index of the heaviest bin passing `allowed`, if any bin has weight.
fn heaviest(weights: &[f32; HUE_BINS], allowed: impl Fn(usize) -> bool) -> Option<usize> {
    weights
        .iter()
        .enumerate()
        .filter(|(bin, weight)| **weight > 0.0 && allowed(*bin))
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(bin, _)| bin)
}

/// Distance between two hue bins, the short way round the circle.
fn bin_distance(a: usize, b: usize) -> usize {
    let raw = a.abs_diff(b);
    raw.min(HUE_BINS - raw)
}

/// The representative colour of a bin: its centre hue at the members' median
/// saturation and lightness.
fn bin_colour(bin: usize, members: &[(f32, f32)]) -> (u8, u8, u8) {
    let hue = (bin as f32 + 0.5) * (360.0 / HUE_BINS as f32);
    let mut saturations: Vec<f32> = members.iter().map(|(s, _)| *s).collect();
    let mut lightnesses: Vec<f32> = members.iter().map(|(_, l)| *l).collect();
    saturations.sort_by(f32::total_cmp);
    lightnesses.sort_by(f32::total_cmp);
    let median = |values: &[f32], fallback: f32| {
        if values.is_empty() {
            fallback
        } else {
            values[values.len() / 2]
        }
    };
    hsl_to_rgb(hue, median(&saturations, 0.6), median(&lightnesses, 0.55))
}

/// Lift a colour until it is comfortably legible on `background`.
///
/// Every field the palette feeds is drawn as a foreground — borders, titles,
/// the progress bar, the visualiser gradient — so contrast is the constraint
/// that matters. Saturation is clamped in the same pass.
pub fn ensure_readable(colour: (u8, u8, u8), background: Color) -> (u8, u8, u8) {
    let (hue, saturation, lightness) = rgb_to_hsl(colour.0, colour.1, colour.2);
    let saturation = saturation.clamp(ACCENT_SATURATION.0, ACCENT_SATURATION.1);

    let background_luminance = match background {
        Color::Rgb(r, g, b) => relative_luminance(r, g, b),
        // Unknown to us: the terminal owns it. Assume dark.
        _ => ASSUMED_BACKGROUND,
    };
    // On a light background we have to go darker, not brighter.
    let darken = background_luminance > 0.5;

    // Walk lightness in small steps rather than solving analytically: hue is
    // fixed, so the luminance curve is not invertible in closed form, and 100
    // steps is nothing at this scale.
    let mut best = hsl_to_rgb(hue, saturation, lightness);
    for step in 0..=100 {
        let candidate_lightness = if darken {
            lightness - step as f32 * 0.01
        } else {
            lightness + step as f32 * 0.01
        };
        if !(0.0..=1.0).contains(&candidate_lightness) {
            break;
        }
        best = hsl_to_rgb(hue, saturation, candidate_lightness);
        if contrast(
            relative_luminance(best.0, best.1, best.2),
            background_luminance,
        ) >= MIN_CONTRAST
        {
            break;
        }
    }
    best
}

impl Theme {
    /// This theme with its accent colours taken from cover art.
    ///
    /// Backgrounds, body text, dim text and errors are left alone: those decide
    /// whether the app is readable, and no artwork should get a vote on that.
    ///
    /// Fields whose preset value is not an RGB colour are also left alone. The
    /// `terminal` preset is built entirely from indexed colours precisely so it
    /// follows the user's terminal palette, and overwriting them with RGB would
    /// quietly defeat that.
    pub fn tinted(&self, palette: &Palette) -> Theme {
        let background = self.background.0;
        let primary = ThemeColor(rgb(ensure_readable(palette.primary, background)));
        let secondary = ThemeColor(rgb(ensure_readable(palette.secondary, background)));

        let mut tinted = self.clone();
        for (field, value) in [
            (&mut tinted.accent, primary),
            (&mut tinted.border_focused, primary),
            (&mut tinted.current_track, primary),
            (&mut tinted.progress, primary),
            (&mut tinted.viz_high, primary),
            (&mut tinted.viz_low, secondary),
        ] {
            if matches!(field.0, Color::Rgb(..)) {
                *field = value;
            }
        }
        tinted
    }
}

fn rgb((r, g, b): (u8, u8, u8)) -> Color {
    Color::Rgb(r, g, b)
}

// ---- colour space helpers ----------------------------------------------

/// Hue in degrees, saturation and lightness in `0..=1`.
fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let lightness = (max + min) / 2.0;
    let delta = max - min;

    if delta <= f32::EPSILON {
        return (0.0, 0.0, lightness);
    }

    let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
    let hue = if max == r {
        60.0 * (((g - b) / delta) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    ((hue + 360.0) % 360.0, saturation.clamp(0.0, 1.0), lightness)
}

fn hsl_to_rgb(hue: f32, saturation: f32, lightness: f32) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let h = (hue % 360.0 + 360.0) % 360.0 / 60.0;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = lightness - c / 2.0;
    let to_byte = |v: f32| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
    (to_byte(r), to_byte(g), to_byte(b))
}

/// WCAG relative luminance.
fn relative_luminance(r: u8, g: u8, b: u8) -> f32 {
    let channel = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

/// WCAG contrast ratio between two relative luminances.
fn contrast(a: f32, b: f32) -> f32 {
    let (lighter, darker) = if a > b { (a, b) } else { (b, a) };
    (lighter + 0.05) / (darker + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode a solid image so `extract` sees the same bytes it would get from
    /// a server.
    fn encoded(pixel: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
        let image = image::RgbImage::from_fn(64, 64, |x, y| image::Rgb(pixel(x, y)));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(image)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    #[test]
    fn solid_red_cover_yields_a_red_accent() {
        let palette = extract(&encoded(|_, _| [200, 30, 30])).unwrap();
        let (hue, saturation, _) =
            rgb_to_hsl(palette.primary.0, palette.primary.1, palette.primary.2);
        assert!(
            !(25.0..=335.0).contains(&hue),
            "expected a red hue, got {hue}"
        );
        assert!(saturation > 0.3);
    }

    #[test]
    fn greyscale_cover_yields_no_palette() {
        assert!(extract(&encoded(|x, _| [(x * 4) as u8; 3])).is_none());
    }

    #[test]
    fn two_colour_cover_separates_primary_from_secondary() {
        // Two thirds red, one third blue: red wins, blue becomes the secondary.
        let palette = extract(&encoded(
            |x, _| {
                if x < 42 { [200, 30, 30] } else { [30, 30, 200] }
            },
        ))
        .unwrap();
        let (primary, ..) = rgb_to_hsl(palette.primary.0, palette.primary.1, palette.primary.2);
        let (secondary, ..) = rgb_to_hsl(
            palette.secondary.0,
            palette.secondary.1,
            palette.secondary.2,
        );
        assert!(!(25.0..=335.0).contains(&primary), "primary hue {primary}");
        assert!(
            (200.0..280.0).contains(&secondary),
            "secondary hue {secondary}"
        );
    }

    #[test]
    fn accents_are_legible_on_the_theme_background() {
        let background = Color::Rgb(0x1a, 0x1b, 0x26); // tokyo_night
        let background_luminance = relative_luminance(0x1a, 0x1b, 0x26);
        // A very dark colour of every hue must be lifted clear of the floor.
        for hue in (0..360).step_by(15) {
            let dark = hsl_to_rgb(hue as f32, 0.7, 0.06);
            let fixed = ensure_readable(dark, background);
            let ratio = contrast(
                relative_luminance(fixed.0, fixed.1, fixed.2),
                background_luminance,
            );
            assert!(ratio >= MIN_CONTRAST, "hue {hue} only reached {ratio}");
        }
    }

    #[test]
    fn accents_are_legible_on_a_light_background() {
        let background = Color::Rgb(0xff, 0xff, 0xff);
        let background_luminance = relative_luminance(0xff, 0xff, 0xff);
        for hue in (0..360).step_by(15) {
            let bright = hsl_to_rgb(hue as f32, 0.7, 0.95);
            let fixed = ensure_readable(bright, background);
            let ratio = contrast(
                relative_luminance(fixed.0, fixed.1, fixed.2),
                background_luminance,
            );
            assert!(ratio >= MIN_CONTRAST, "hue {hue} only reached {ratio}");
        }
    }

    #[test]
    fn tinting_replaces_accents_but_not_the_background() {
        let theme = Theme::tokyo_night();
        let palette = Palette {
            primary: (200, 30, 30),
            secondary: (30, 30, 200),
        };
        let tinted = theme.tinted(&palette);
        assert_eq!(tinted.background.0, theme.background.0);
        assert_eq!(tinted.foreground.0, theme.foreground.0);
        assert_eq!(tinted.dim.0, theme.dim.0);
        assert_eq!(tinted.error.0, theme.error.0);
        assert_ne!(tinted.accent.0, theme.accent.0);
        assert_eq!(tinted.accent.0, tinted.progress.0);
        assert_eq!(tinted.accent.0, tinted.border_focused.0);
        assert_ne!(tinted.viz_low.0, tinted.viz_high.0);
    }

    #[test]
    fn the_ansi_preset_is_never_tinted() {
        // Its whole purpose is to follow the terminal's own palette.
        let theme = Theme::terminal_ansi();
        let tinted = theme.tinted(&Palette {
            primary: (200, 30, 30),
            secondary: (30, 30, 200),
        });
        assert_eq!(
            toml::to_string(&tinted).unwrap(),
            toml::to_string(&theme).unwrap()
        );
    }
}

/// Eyeballing the extractor against a real library.
///
/// Synthetic images prove the maths; only actual sleeve art shows whether the
/// colours it picks are the ones a person would have picked. Ignored by default
/// because it depends on a populated cover cache.
#[cfg(test)]
mod diagnostic {
    use super::*;

    /// Print every cached cover's derived accents as truecolor swatches:
    ///
    /// ```text
    /// cargo test --release -- --ignored --nocapture palette_swatches
    /// ```
    ///
    /// Use `--release`: a debug build spends ~300 ms per cover in the JPEG
    /// decoder, which says nothing useful about the real cost.
    #[test]
    #[ignore]
    fn palette_swatches() {
        let dir = directories::BaseDirs::new()
            .unwrap()
            .cache_dir()
            .join("wander/covers");
        let mut none = 0;
        let mut total = 0;
        let start = std::time::Instant::now();
        for entry in std::fs::read_dir(&dir).unwrap().flatten().take(400) {
            let Ok(bytes) = std::fs::read(entry.path()) else {
                continue;
            };
            total += 1;
            let name = entry.file_name().to_string_lossy().to_string();
            match extract(&bytes) {
                None => {
                    none += 1;
                    println!("{name:44}  (no colour)");
                }
                Some(p) => {
                    let bg = Color::Reset;
                    let a = ensure_readable(p.primary, bg);
                    let b = ensure_readable(p.secondary, bg);
                    println!(
                        "{name:44}  \x1b[48;2;{};{};{}m    \x1b[0m \x1b[48;2;{};{};{}m    \x1b[0m  \
                         raw #{:02x}{:02x}{:02x}/#{:02x}{:02x}{:02x} -> #{:02x}{:02x}{:02x}/#{:02x}{:02x}{:02x}",
                        a.0,
                        a.1,
                        a.2,
                        b.0,
                        b.1,
                        b.2,
                        p.primary.0,
                        p.primary.1,
                        p.primary.2,
                        p.secondary.0,
                        p.secondary.1,
                        p.secondary.2,
                        a.0,
                        a.1,
                        a.2,
                        b.0,
                        b.1,
                        b.2,
                    );
                }
            }
        }
        println!("\n{none}/{total} covers had no usable colour");
        println!("{:?} per cover", start.elapsed() / total.max(1));
    }
}
