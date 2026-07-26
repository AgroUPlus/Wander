mod app;
mod config;
mod history;
mod integrations;
mod keymap;
mod library;
mod paths;
mod player;
mod subsonic;
mod theme;
mod ui;

use anyhow::{Context, Result, bail};
use crossterm::event::{Event, EventStream, KeyEventKind};
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use app::App;
use subsonic::SubsonicClient;
use ui::cover::CoverRenderer;

/// Redraw cadence while something on screen is moving. A still UI does not
/// redraw at all; it waits for an event instead.
const TICK: Duration = Duration::from_millis(50);

#[tokio::main]
async fn main() -> Result<()> {
    let config = config::Config::load()?;
    // Problems that used to be fatal are now shown in the status line, since
    // the settings panel can fix them without a restart.
    let mut startup_warning: Option<String> = None;

    // `wander --set-password` stores the server password in the OS keyring.
    // The password is read from stdin rather than argv so it never reaches
    // shell history or another user's `ps` output.
    if std::env::args().any(|arg| arg == "--set-password") {
        return set_password(&config);
    }

    if let Some(path) = std::env::args()
        .skip_while(|a| a != "--decode-check")
        .nth(1)
    {
        return decode_check(&path);
    }

    if let Some(path) = std::env::args().skip_while(|a| a != "--scan-check").nth(1) {
        return scan_check(&path);
    }

    // One library for the whole process. Both backends are optional and can be
    // swapped in later from the settings panel, so an unconfigured start is not
    // an error any more — it opens the setup wizard instead.
    let library = Arc::new(library::MergedLibrary::new());

    match build_remote(&config) {
        Ok(Some(remote)) => {
            // Report a bad server before taking over the terminal, but do not
            // exit: the user can fix the credentials in Settings.
            if let Err(err) = remote.client().ping().await {
                startup_warning = Some(format!("could not reach the server: {err:#}"));
            }
            library.set_remote(Some(remote));
        }
        Ok(None) => {}
        Err(err) => startup_warning = Some(format!("server configuration: {err:#}")),
    }

    let config_scan_on_start = config.local.scan_on_start && !config.local.paths.is_empty();

    if !config.local.paths.is_empty() {
        library.set_local(Some(Arc::new(library::LocalLibrary::from_cache(
            config.local.playlist_dir.clone(),
        ))));
    }

    let (player, mut audio) = player::spawn(
        Arc::clone(&library) as Arc<dyn library::Library>,
        config.buffer_seconds,
    )?;

    // Take the visualiser tap; the rest of `audio` stays alive so the device
    // does, since dropping it would stop playback.
    let (_, placeholder) = rtrb::RingBuffer::<f32>::new(1);
    let tap = std::mem::replace(&mut audio.tap, placeholder);
    let mut spectrum = player::spectrum::Spectrum::new(tap, player.shared.sample_rate(), 32);

    let discord_config = config.discord.clone();

    let (load_tx, mut load_rx) = mpsc::unbounded_channel();

    // Must happen before entering the alternate screen: this writes a terminal
    // query to stdout and reads the reply. It also starts the encoder thread,
    // which reports finished cover art through the same load channel.
    let mut covers = CoverRenderer::detect(load_tx.clone());

    let mut app = App::new(
        config,
        Arc::clone(&library) as Arc<dyn library::Library>,
        player,
        load_tx,
    )?;
    app.library_root = Some(Arc::clone(&library));
    if let Some(warning) = startup_warning {
        app.status_message = Some(warning);
    }
    app.refresh_password_state();
    app.bootstrap();
    // With nothing configured, the old build exited here with instructions to
    // hand-write a config file. Now it asks instead.
    app.maybe_start_setup();
    if config_scan_on_start {
        app.rescan_local_library();
    }

    // Publish to MPRIS so the desktop bar, lock screen, playerctl and media
    // keys all see us. A missing session bus must not stop playback, so a
    // failure here is only reported in the status line.
    if let Err(err) =
        integrations::mpris::spawn(app.player.clone(), std::sync::Arc::clone(&app.covers)).await
    {
        app.status_message = Some(format!("MPRIS unavailable: {err:#}"));
    }

    // Rich Presence is opt-in and must never interfere with playback, so a
    // misconfiguration is surfaced in the status line rather than being fatal.
    match integrations::discord::spawn(
        app.player.clone(),
        Arc::clone(&library) as Arc<dyn library::Library>,
        discord_config,
    ) {
        Ok(diagnostic) => app.discord_diagnostic = Some(diagnostic),
        Err(err) => app.status_message = Some(format!("Discord: {err:#}")),
    }

    // ratatui::init installs a panic hook that restores the terminal, so a
    // crash cannot leave the user with an unusable shell.
    let mut terminal = ratatui::init();
    // Mouse capture is not part of ratatui::init, so it is enabled and torn
    // down here. Without the matching disable, the terminal keeps emitting
    // mouse escape sequences after we exit.
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture);

    let result = run(
        &mut terminal,
        &mut app,
        &mut covers,
        &mut spectrum,
        &mut load_rx,
    )
    .await;

    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    ratatui::restore();

    result
}

/// Build the Subsonic backend from the config, if one is configured.
///
/// `Ok(None)` means the user has not set up a server — a normal state now that
/// a local-only library is supported.
pub fn build_remote(config: &config::Config) -> Result<Option<Arc<library::SubsonicLibrary>>> {
    if !config.server.enabled
        || config.server.url.trim().is_empty()
        || config.server.username.trim().is_empty()
    {
        return Ok(None);
    }
    let password = config.password()?;
    let client = SubsonicClient::new(
        &config.server.url,
        &config.server.username,
        &password,
        config.server.format.as_deref(),
    )?;
    Ok(Some(Arc::new(library::SubsonicLibrary::new(client))))
}

/// Decode a local audio file through the real playback pipeline.
fn decode_check(path: &str) -> Result<()> {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    let file = std::fs::File::open(path).with_context(|| format!("opening {path}"))?;
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_string());

    let mut output = player::output::AudioOutput::new(1.0)?;
    let shared = Arc::clone(&output.shared);
    shared.set_volume(0.0);
    let cancel = Arc::new(AtomicBool::new(false));

    eprintln!(
        "device: {} Hz, {} ch",
        shared.sample_rate(),
        shared.channels()
    );

    let outcome = player::decoder::decode_stream(
        Box::new(file),
        &mut output.producer,
        &shared,
        &cancel,
        ext.as_deref(),
        std::time::Duration::ZERO,
    )?;

    eprintln!(
        "decoded ok: {outcome:?}, {} frames = {:.1}s, {} underruns",
        shared.frames_played(),
        shared.frames_played() as f64 / shared.sample_rate() as f64,
        shared.underrun_frames()
    );
    Ok(())
}

/// Scan a folder and print what the local library made of it.
///
/// The counterpart to `--decode-check`: that one answers "can wander play this
/// file", this one answers "did wander read its tags the way I expect", which
/// is the other half of why a local track shows up wrong.
fn scan_check(path: &str) -> Result<()> {
    let roots = vec![std::path::PathBuf::from(path)];
    let index = library::local::scan::scan(
        &roots,
        &library::local::index::LocalIndex::default(),
        |count| {
            if count % 100 == 0 {
                eprint!("\rscanned {count} files…");
            }
        },
    );

    let albums = index.albums();
    eprintln!(
        "\r{} songs, {} albums, {} artists, {} genres",
        index.tracks.len(),
        albums.len(),
        index.artists().len(),
        index.genres().len()
    );

    for album in albums.iter().take(20) {
        eprintln!(
            "  {} — {} ({}) [{} tracks]",
            album.artist.as_deref().unwrap_or("Unknown Artist"),
            album.name,
            album
                .year
                .map(|y| y.to_string())
                .unwrap_or_else(|| "no year".into()),
            album.song_count
        );
    }
    if albums.len() > 20 {
        eprintln!("  … and {} more", albums.len() - 20);
    }

    let untagged = index.tracks.iter().filter(|t| t.album.is_none()).count();
    if untagged > 0 {
        eprintln!("\n{untagged} file(s) had no album tag and were filed under Unknown Album.");
    }
    Ok(())
}

/// Read a password from stdin and store it in the OS keyring.
fn set_password(config: &config::Config) -> Result<()> {
    if config.server.username.is_empty() {
        bail!(
            "set `username` under [server] in {} first",
            config::Config::path()?.display()
        );
    }

    eprint!("Password for {}: ", config.server.username);
    let mut password = String::new();
    std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut password)
        .context("reading password from stdin")?;
    let password = password.trim_end_matches(['\n', '\r']);

    if password.is_empty() {
        bail!("no password provided");
    }

    paths::store_keyring_password(&config.server.username, password)?;

    eprintln!(
        "\nStored password for {} in the keyring.",
        config.server.username
    );
    Ok(())
}

async fn run(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    covers: &mut CoverRenderer,
    spectrum: &mut player::spectrum::Spectrum,
    loads: &mut mpsc::UnboundedReceiver<app::LoadEvent>,
) -> Result<()> {
    let mut events = EventStream::new();
    // Rebuilt every frame by `ui::draw`, then used to route the next mouse
    // event, so clicks always match what is currently on screen.
    let mut hits = ui::Hits::default();

    loop {
        app.sync_cover();
        // Deferred disk writes land here, at most once a second, so nothing in
        // the input path ever blocks on the filesystem.
        app.flush_state(false);
        terminal.draw(|frame| ui::draw(frame, app, covers, spectrum, &mut hits))?;

        if app.should_quit {
            app.flush_state(true);
            return Ok(());
        }

        // While playing, wake on a timer so the clock and seek bar advance.
        // While idle, block until something actually happens.
        let tick = async {
            if app.is_animating() {
                tokio::time::sleep(TICK).await;
            } else {
                std::future::pending::<()>().await;
            }
        };

        tokio::select! {
            event = events.next() => match event {
                Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                    // Any keypress dismisses a transient status message.
                    app.status_message = None;
                    app.handle_key(key);
                }
                Some(Ok(Event::Mouse(mouse))) => app.handle_mouse(mouse, &hits),
                // Resize and other events just trigger a redraw.
                Some(Ok(_)) => {}
                Some(Err(err)) => return Err(err).context("reading terminal events"),
                None => return Ok(()),
            },
            Some(event) = loads.recv() => app.apply(event),
            _ = tick => {}
        }
    }
}
