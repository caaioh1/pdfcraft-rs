//! Help ▸ Check for updates (issue #28): ask for the latest release, offer its download page and,
//! when the app can, download and run its installer.
//!
//! The desktop app supplies how to ask ([`PdfCraftApp::update_source`]) and how to install
//! ([`PdfCraftApp::install_source`]), so this crate has no network code; without a source (the web
//! build, tests) the command opens the releases page.
//! In this fork the app also asks once at start ([`PdfCraftApp::check_for_updates_at_start`]),
//! quietly: the dialog opens only when a newer release exists. Nothing is installed without the
//! user pressing Install now.

use std::sync::Arc;

use egui::{Align, Layout};

use crate::{PdfCraftApp, theme, widgets};

/// Where every release of this fork is listed.
pub const RELEASES_PAGE: &str = "https://github.com/caaioh1/pdfcraft-rs/releases";

/// The installer a release carries, as the app found it on the release's page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installer {
    /// The installer's file name, such as `pdfcraft-0.9.0-windows-x64.msi`.
    pub file_name: String,
    /// Where it is downloaded from.
    pub msi_url: String,
    /// The release's checksum list (`SHA256SUMS.txt`), which the installer is checked against.
    pub sums_url: String,
}

/// The latest published release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// Its version tag, such as `v0.2.0`.
    pub version: String,
    /// Its page on [`RELEASES_PAGE`], where the downloads are.
    pub url: String,
    /// Its Windows installer, when the release has one the app can verify.
    pub installer: Option<Installer>,
}

/// Asks for the latest release (blocking; it runs on its own thread).
pub type UpdateSource = Arc<dyn Fn() -> Result<Release, String> + Send + Sync>;

/// Downloads a release's installer, verifies it and starts it (blocking; it runs on its own
/// thread). It returns once the installer has started.
pub type InstallSource = Arc<dyn Fn(&Release) -> Result<(), String> + Send + Sync>;

/// Starts PdfCraft again once this one has exited, so the new version opens in its place. The
/// app closes itself right after (asking about unsaved work as for any close).
pub type RestartSource = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

/// Whether release `latest` (a tag such as `v0.2.0`) is newer than version `current` (`0.1.1`).
/// Pre-release and build suffixes are ignored; a version that doesn't parse is never newer.
pub fn is_newer(latest: &str, current: &str) -> bool {
    matches!((parse(latest), parse(current)), (Some(l), Some(c)) if l > c)
}

fn parse(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let core = v.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let mut next = |required: bool| match parts.next() {
        Some(p) => p.parse::<u64>().ok(),
        None if required => None,
        None => Some(0),
    };
    let version = (next(true)?, next(false)?, next(false)?);
    parts.next().is_none().then_some(version)
}

/// Where a check is.
#[derive(Default)]
pub(crate) enum Check {
    #[default]
    Idle,
    #[cfg(not(target_arch = "wasm32"))]
    Running(std::sync::mpsc::Receiver<Result<Release, String>>),
    Done(Result<Release, String>),
}

/// Where installing the update is.
#[derive(Default)]
pub(crate) enum Install {
    #[default]
    Idle,
    #[cfg(not(target_arch = "wasm32"))]
    Running(std::sync::mpsc::Receiver<Result<(), String>>),
    /// The installer finished: the new version is on disk and the running one is still the old.
    Done,
    /// Restart now was pressed once: the helper that opens the new version is waiting for this
    /// window to close. Pressing it again would start a second helper, so it is not offered.
    Restarting,
    Failed(String),
}

#[derive(Default)]
pub(crate) struct Updates {
    pub(crate) check: Check,
    pub(crate) install: Install,
    /// The Updates dialog is showing.
    pub(crate) open: bool,
    /// The running check was made at start: it opens the dialog only for a newer release.
    pub(crate) quiet: bool,
}

impl PdfCraftApp {
    /// Help ▸ Check for updates: ask for the latest release and show the outcome.
    pub fn check_for_updates(&mut self) {
        self.updates.quiet = false;
        self.start_update_check(true);
    }

    /// Ask for the latest release once, when the app starts, without showing anything unless a
    /// newer one exists. A failed check (offline, rate-limited) stays silent.
    pub fn check_for_updates_at_start(&mut self) {
        if self.update_source.is_some() && matches!(self.updates.check, Check::Idle) {
            self.updates.quiet = true;
            self.start_update_check(false);
        }
    }

    fn start_update_check(&mut self, show: bool) {
        let Some(source) = self.update_source.clone() else {
            self.open_url(RELEASES_PAGE);
            return;
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.updates.open |= show;
            if matches!(self.updates.check, Check::Running(_)) {
                return;
            }
            let (tx, rx) = std::sync::mpsc::channel();
            let ctx = self.ctx.clone();
            std::thread::spawn(move || {
                // The receiver may be gone (the app quit): nothing to report to then.
                let _ = tx.send(source());
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            });
            self.updates.check = Check::Running(rx);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (source, show);
            self.open_url(RELEASES_PAGE);
        }
    }

    /// Download the release's installer, verify it and run it (Install now).
    fn start_install(&mut self, release: Release) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(install) = self.install_source.clone() else {
                return;
            };
            if matches!(self.updates.install, Install::Running(_)) {
                return;
            }
            let (tx, rx) = std::sync::mpsc::channel();
            let ctx = self.ctx.clone();
            std::thread::spawn(move || {
                // The receiver may be gone (the app quit): nothing to report to then.
                let _ = tx.send(install(&release));
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            });
            self.updates.install = Install::Running(rx);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = release;
        }
    }

    /// Pick up a finished check or installation (each frame).
    pub(crate) fn poll_updates(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Check::Running(rx) = &self.updates.check {
                let result = match rx.try_recv() {
                    Ok(r) => r,
                    Err(std::sync::mpsc::TryRecvError::Empty) => return,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("the update check stopped unexpectedly".into()),
                };
                // A quiet check opens the dialog only to announce a newer release.
                if self.updates.quiet && matches!(&result, Ok(r) if is_newer(&r.version, env!("CARGO_PKG_VERSION"))) {
                    self.updates.open = true;
                }
                self.updates.quiet = false;
                self.updates.check = Check::Done(result);
            }
            if let Install::Running(rx) = &self.updates.install {
                self.updates.install = match rx.try_recv() {
                    Ok(Ok(())) => Install::Done,
                    Ok(Err(e)) => Install::Failed(e),
                    Err(std::sync::mpsc::TryRecvError::Empty) => return,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => Install::Failed("the installation stopped unexpectedly".into()),
                };
            }
        }
    }
}

/// The Updates dialog.
pub(crate) fn dialog(app: &mut PdfCraftApp, ctx: &egui::Context) {
    if !app.updates.open {
        return;
    }
    let t = theme::Tokens::get(ctx);
    let current = env!("CARGO_PKG_VERSION");
    let mut close = false;
    let mut download: Option<String> = None;
    // The release to install, offered while an installer is available and none is running.
    let mut install: Option<Release> = None;
    let mut install_now = false;
    let mut restart = false;
    let modal = crate::widgets::show_modal(ctx, "updates", |ui| {
        ui.set_width(420.0);
        ui.horizontal(|ui| {
            ui.add(crate::icons::image("cloud", 22.0, t.accent));
            ui.label(egui::RichText::new(tl!("Check for updates")).font(theme::semibold(16.0)));
        });
        ui.add_space(8.0);
        match &app.updates.check {
            Check::Idle => {
                ui.label(tl!("No check has run yet."));
            }
            #[cfg(not(target_arch = "wasm32"))]
            Check::Running(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tl!("Checking for a newer version…"));
                });
            }
            Check::Done(Ok(r)) if is_newer(&r.version, current) => {
                let version = r.version.trim_start_matches(['v', 'V']);
                ui.label(egui::RichText::new(crate::i18n::fmt(tl!("PdfCraft {v} is available."), &[("v", version)])).strong());
                ui.label(
                    egui::RichText::new(crate::i18n::fmt(
                        tl!("You have version {c}. Download the new version from its release page."),
                        &[("c", current)],
                    ))
                    .color(t.text_muted),
                );
                download = Some(r.url.clone());
                match &app.updates.install {
                    Install::Idle => {}
                    #[cfg(not(target_arch = "wasm32"))]
                    Install::Running(_) => {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(tl!("Downloading the update…"));
                        });
                    }
                    Install::Done | Install::Restarting => {
                        ui.add_space(6.0);
                        ui.label(tl!("PdfCraft is updated. Restart to use the new version."));
                    }
                    Install::Failed(e) => {
                        ui.add_space(6.0);
                        ui.label(crate::i18n::fmt(tl!("Couldn't install the update: {e}"), &[("e", &e.to_string())]));
                    }
                }
                let can_install =
                    app.install_source.is_some() && r.installer.is_some() && matches!(app.updates.install, Install::Idle | Install::Failed(_));
                if can_install {
                    install = Some(r.clone());
                }
            }
            Check::Done(Ok(_)) => {
                ui.label(crate::i18n::fmt(tl!("PdfCraft {c} is up to date."), &[("c", current)]));
            }
            Check::Done(Err(e)) => {
                ui.label(crate::i18n::fmt(tl!("Couldn't check for updates: {e}"), &[("e", &e.to_string())]));
                ui.label(
                    egui::RichText::new(crate::i18n::fmt(
                        tl!("You have version {c}. All releases are listed at {page}."),
                        &[("c", current), ("page", RELEASES_PAGE)],
                    ))
                    .color(t.text_muted),
                );
            }
        }
        ui.add_space(10.0);
        ui.label(
            egui::RichText::new(tl!("Asks GitHub for the latest release. Nothing is downloaded or installed automatically."))
                .color(t.text_muted)
                .small(),
        );
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if let Some(url) = download.take() {
                let updated = matches!(app.updates.install, Install::Done | Install::Restarting) && app.restart_source.is_some();
                if updated {
                    // One press only: a second would start a second helper and open a second window.
                    let ready = matches!(app.updates.install, Install::Done);
                    ui.add_enabled_ui(ready, |ui| {
                        restart = widgets::pill_button(ui, tl!("Restart now"), true).clicked();
                    });
                }
                if install.is_some() {
                    install_now = widgets::pill_button(ui, tl!("Install now"), true).clicked();
                }
                let get = widgets::pill_button(ui, tl!("Download"), install.is_none() && !updated).clicked();
                let later = widgets::pill_button(ui, tl!("Later"), false).clicked();
                // Install now keeps the dialog open to show the download's progress.
                close = get || later;
                download = get.then_some(url);
            } else if widgets::pill_button(ui, tl!("Close"), true).clicked() {
                close = true;
            }
        });
    });
    if modal.should_close() {
        close = true;
        download = None;
    }
    if install_now && let Some(release) = install {
        app.start_install(release);
    }
    if restart
        && matches!(app.updates.install, Install::Done)
        && let Some(source) = app.restart_source.clone()
    {
        match source() {
            Ok(()) => {
                // The dialog stays up until the window goes; if closing is held back (unsaved
                // work), the usual save prompt asks first and the helper keeps waiting.
                app.updates.install = Install::Restarting;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => app.updates.install = Install::Failed(format!("couldn't restart ({e})")),
        }
    }
    if close {
        app.updates.open = false;
        if let Some(url) = download {
            app.open_url(&url);
        }
    }
}
