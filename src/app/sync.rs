//! Driving library sync from the app.
//!
//! Follows `rescan_local_library`'s shape: register an [`Operation`] so the progress pane has
//! something to show, do the work off the UI thread, and report back through a [`LoadEvent`].
//!
//! A pass is deliberately bounded by `config.sync.hash_batch` and `upload_batch`. Hashing reads
//! every byte of every file and uploading sends them; doing a whole library in one go would peg
//! the disk and the network for as long as it took, with no way to stop it. Each pass makes
//! progress and reports what is left.

use crate::app::types::{
    LoadEvent, NotificationLevel, Operation, OperationKind, OperationStatus, SyncSummary,
};
use crate::app::App;
use crate::integrations::sync::{hash_file, SyncClient, UploadOutcome};
use crate::ui::overlay::{Overlay, SyncState};

/// How many missing tracks to ask about at once.
const OFFER_LIMIT: i64 = 200;

impl App {
    /// Whether Agro is paired and sync is configured at all.
    fn sync_client(&self) -> Option<SyncClient> {
        let agro = &self.config.agro;
        if !agro.enabled || agro.passphrase.trim().is_empty() || agro.server.trim().is_empty() {
            return None;
        }
        SyncClient::new(&agro.server, &agro.username, &agro.passphrase, &agro.device_id).ok()
    }

    /// Hashes, reports and (if enabled) uploads one batch.
    pub fn sync_library(&mut self) {
        let Some(client) = self.sync_client() else {
            self.push_notification(NotificationLevel::Warning, "Pair with Agro first");
            return;
        };
        let Some(local) = self.library_root.as_ref().and_then(|root| root.local()) else {
            self.push_notification(NotificationLevel::Warning, "No local music folder configured");
            return;
        };

        let config = self.config.sync.clone();
        if !config.enabled && !config.report_holdings {
            self.push_notification(NotificationLevel::Warning, "Library sync is switched off");
            return;
        }

        self.add_operation(Operation {
            id: "library-sync".into(),
            title: "Library sync".into(),
            kind: OperationKind::Sync,
            progress: None,
            status: OperationStatus::Running,
            details: Some("Hashing local files...".into()),
            started_at: std::time::Instant::now(),
        });

        let loads = self.loads.clone();
        tokio::spawn(async move {
            let outcome = run_pass(client, local, config).await;
            let _ = loads.send(LoadEvent::SyncFinished(outcome.map_err(|e| format!("{e:#}"))));
        });
    }

    /// Asks the server what this machine is missing, and offers it.
    pub fn check_sync_offers(&mut self) {
        let Some(client) = self.sync_client() else { return };
        self.spawn_load(async move {
            let missing = client.missing_here(OFFER_LIMIT).await.unwrap_or_default();
            Ok(LoadEvent::SyncOffer(missing))
        });
    }

    /// The user accepted the offer: pull the files down into the local library.
    ///
    /// They land in the **first** configured music folder, filed by artist and album — the same
    /// layout Agro used on the server, so the local scanner sorts them alongside everything else.
    /// Choosing between several roots is a question worth asking only if someone has several, and
    /// the first is the one every existing feature already treats as primary.
    pub(crate) fn accept_sync_offer(&mut self) {
        let Some(dir) = self.config.local.paths.first().cloned() else {
            if let Some(Overlay::Sync(state)) = self.overlay.as_mut() {
                state.result = Some(Err("Add a music folder first".to_string()));
            }
            return;
        };
        let Some(client) = self.sync_client() else { return };

        let Some(Overlay::Sync(state)) = self.overlay.as_mut() else {
            return;
        };
        state.pending = true;
        let missing = state.missing.clone();

        let loads = self.loads.clone();
        tokio::spawn(async move {
            let mut fetched = 0usize;
            let mut failure = None;
            for track in &missing {
                match client.fetch(track, &dir).await {
                    Ok(_) => fetched += 1,
                    Err(error) => {
                        // Stop at the first failure rather than grinding through hundreds of
                        // doomed requests — whatever went wrong is very unlikely to be per-file.
                        failure = Some(format!("{error:#}"));
                        break;
                    }
                }
            }
            let result = match failure {
                Some(error) if fetched == 0 => Err(error),
                // Partial success is still success: the files that arrived are real, and the
                // remainder is offered again next time.
                _ => Ok(fetched),
            };
            let _ = loads.send(LoadEvent::SyncFetched(result));
        });
    }

    /// Called when a fetch finishes. Rescans so the new files appear without a restart.
    pub(crate) fn on_sync_fetched(&mut self, result: Result<usize, String>) {
        if let Some(Overlay::Sync(state)) = self.overlay.as_mut() {
            state.pending = false;
            state.result = Some(result.clone());
        }
        if matches!(result, Ok(count) if count > 0) {
            self.rescan_local_library();
        }
    }

    /// Called from the load dispatcher when a pass finishes.
    pub(crate) fn on_sync_finished(&mut self, result: Result<SyncSummary, String>) {
        self.finish_operation(
            "library-sync",
            match &result {
                Ok(_) => OperationStatus::Completed,
                Err(error) => OperationStatus::Failed(error.clone()),
            },
        );
        match result {
            Ok(summary) => {
                let message = if summary.uploaded == 0 && summary.hashed == 0 {
                    "Library already in sync".to_string()
                } else {
                    format!(
                        "Synced {} track{}{}",
                        summary.uploaded,
                        if summary.uploaded == 1 { "" } else { "s" },
                        if summary.remaining > 0 {
                            format!(" — {} still to go", summary.remaining)
                        } else {
                            String::new()
                        }
                    )
                };
                self.push_notification(NotificationLevel::Info, message);
            }
            Err(error) => self.push_notification(NotificationLevel::Error, error),
        }
    }

    /// Called when the server answers "what am I missing".
    pub(crate) fn on_sync_offer(&mut self, missing: Vec<crate::integrations::sync::MissingTrack>) {
        // Nothing missing, or the user is in the middle of something else — an offer is not worth
        // stealing a modal for.
        if missing.is_empty() || self.overlay.is_some() {
            return;
        }
        self.overlay = Some(Overlay::Sync(SyncState::new(missing)));
    }
}

/// One bounded pass: hash what has not been hashed, report everything, upload what is wanted.
async fn run_pass(
    client: SyncClient,
    local: std::sync::Arc<crate::library::local::LocalLibrary>,
    config: crate::config::SyncConfig,
) -> anyhow::Result<SyncSummary> {
    let mut summary = SyncSummary::default();

    // ── Hash ────────────────────────────────────────────────────────────────────────────────
    // On the blocking pool: this reads whole files and would stall the runtime otherwise.
    let index = local.index();
    let unhashed: Vec<_> = index
        .tracks
        .iter()
        .filter(|t| t.content_hash.is_none())
        .take(config.hash_batch)
        .map(|t| t.path.clone())
        .collect();
    summary.remaining = index
        .tracks
        .iter()
        .filter(|t| t.content_hash.is_none())
        .count()
        .saturating_sub(unhashed.len());

    if !unhashed.is_empty() {
        let hashed = tokio::task::spawn_blocking(move || {
            unhashed
                .into_iter()
                .filter_map(|path| hash_file(&path).ok().map(|hash| (path, hash)))
                .collect::<Vec<_>>()
        })
        .await?;

        summary.hashed = hashed.len();

        // Fold the hashes back into the index and persist, so this is never redone.
        // `index()` returns an Arc, so this works on an owned clone and swaps the whole thing
        // back in — the same copy-on-write the scanner uses.
        let mut index = (*local.index()).clone();
        for (path, hash) in hashed {
            if let Some(track) = index.tracks.iter_mut().find(|t| t.path == path) {
                track.content_hash = Some(hash);
            }
        }
        let _ = index.save();
        local.set_index(index);
    }

    // ── Report ──────────────────────────────────────────────────────────────────────────────
    let index = local.index();
    let hashed: Vec<&crate::library::local::index::LocalTrack> = index
        .tracks
        .iter()
        .filter(|t| t.content_hash.is_some())
        .collect();

    if config.report_holdings && !hashed.is_empty() {
        client.report_holdings(&hashed).await?;
    }

    // ── Upload ──────────────────────────────────────────────────────────────────────────────
    if config.enabled {
        for track in hashed.into_iter().take(config.upload_batch) {
            match client.upload(track).await {
                Ok(UploadOutcome::Uploaded) => summary.uploaded += 1,
                Ok(UploadOutcome::AlreadyPresent) => summary.already_present += 1,
                // Left as-is: the next pass re-declares it and the server hands back an offset, so
                // the transfer continues rather than starting over.
                Ok(UploadOutcome::Partial { .. }) => {}
                Err(error) => return Err(error),
            }
        }
    }

    Ok(summary)
}
