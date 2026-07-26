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
