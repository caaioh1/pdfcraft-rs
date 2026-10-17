//! Help ▸ Check for updates (issue #28), with stand-in release sources (no network).

use std::sync::Arc;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::updates::{InstallSource, Installer, Release, RestartSource, UpdateSource, is_newer};

fn source(answer: Result<&str, &str>) -> UpdateSource {
    let answer = answer.map(str::to_string).map_err(str::to_string);
    Arc::new(move || {
        answer.clone().map(|v| Release { url: format!("https://github.com/caaioh1/pdfcraft-rs/releases/tag/{v}"), version: v, installer: None })
    })
}

fn harness(answer: Result<&str, &str>) -> Harness<'static, PdfCraftApp> {
    Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.update_source = Some(source(answer));
        app
    })
}

/// Run frames until the background check has reported. By what the dialog shows, with the
/// same 30-second deadline as `support::wait_until`, not for a fixed second: under load the
/// check's thread can take longer (#1277).
fn settle(h: &mut Harness<'static, PdfCraftApp>) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        h.run_steps(2);
        if h.query_by_label_contains("Checking for a newer version").is_none() {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "timed out waiting for the update check");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.run_steps(2);
}

#[test]
fn versions_compare_by_number() {
    assert!(is_newer("v0.2.0", "0.1.1"));
    assert!(is_newer("v0.1.10", "0.1.9"));
    assert!(is_newer("1", "0.9.9"));
    assert!(!is_newer("v0.1.1", "0.1.1"));
    assert!(!is_newer("v0.1.0", "0.1.1"));
    assert!(!is_newer("v0.1.1-beta.2", "0.1.1"), "suffixes are ignored");
    assert!(!is_newer("nightly", "0.1.1"), "a tag that isn't a version is never newer");
    assert!(!is_newer("v1.2.3.4", "0.1.1"));
    assert!(!is_newer("v99999999999999999999.0.0", "0.1.1"), "out of range");
}

#[test]
fn a_newer_release_is_offered_for_download() {
    let mut h = harness(Ok("v99.0.0"));
    h.state_mut().execute("help.check_updates");
    settle(&mut h);
    h.get_by_label_contains("PdfCraft 99.0.0 is available");
    h.get_by_label("Download");
    h.get_by_label("Later").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("is available").is_none(), "Later closes the dialog");
}

#[test]
fn an_up_to_date_or_failed_check_says_so() {
    let mut h = harness(Ok(concat!("v", env!("CARGO_PKG_VERSION"))));
    h.state_mut().execute("help.check_updates");
    settle(&mut h);
    h.get_by_label_contains("is up to date");
    assert!(h.query_by_label("Download").is_none());

    let mut h = harness(Err("couldn't reach GitHub"));
    h.state_mut().execute("help.check_updates");
    settle(&mut h);
    h.get_by_label_contains("Couldn't check for updates: couldn't reach GitHub");
}

#[test]
fn nothing_is_asked_until_the_user_checks() {
    // A new app asks nothing by itself: the source is only called from the command (the desktop
    // app starts the quiet check, `check_for_updates_at_start`, from main).
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = calls.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        // Settings from a build that had the startup option are ignored.
        app.restore(r#"{"check_updates_at_start": true}"#);
        let counted = counted.clone();
        app.update_source = Some(Arc::new(move || {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Release { version: "v99.0.0".into(), url: "https://github.com/caaioh1/pdfcraft-rs/releases/tag/v99.0.0".into(), installer: None })
        }));
        app
    });
    settle(&mut h);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(h.query_by_label_contains("is available").is_none());
    h.state_mut().execute("help.check_updates");
    settle(&mut h);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    h.get_by_label_contains("PdfCraft 99.0.0 is available");
}

fn installable(version: &str) -> Release {
    let base = format!("https://github.com/caaioh1/pdfcraft-rs/releases/download/{version}");
    Release {
        version: version.into(),
        url: format!("https://github.com/caaioh1/pdfcraft-rs/releases/tag/{version}"),
        installer: Some(Installer {
            file_name: "pdfcraft-99.0.0-windows-x64.msi".into(),
            msi_url: format!("{base}/pdfcraft-99.0.0-windows-x64.msi"),
            sums_url: format!("{base}/SHA256SUMS.txt"),
        }),
    }
}

fn quiet_harness(answer: Result<Release, &'static str>, install: Option<InstallSource>) -> Harness<'static, PdfCraftApp> {
    quiet_harness_with_restart(answer, install, None)
}

fn quiet_harness_with_restart(
    answer: Result<Release, &'static str>,
    install: Option<InstallSource>,
    restart: Option<RestartSource>,
) -> Harness<'static, PdfCraftApp> {
    Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        let answer = answer.clone().map_err(str::to_string);
        app.update_source = Some(Arc::new(move || answer.clone()));
        app.install_source = install;
        app.restart_source = restart.clone();
        app.check_for_updates_at_start();
        app
    })
}

#[test]
fn the_check_at_start_speaks_only_for_a_newer_release() {
    let mut h = quiet_harness(Ok(installable("v99.0.0")), None);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while h.query_by_label_contains("is available").is_none() {
        h.run_steps(2);
        assert!(std::time::Instant::now() < deadline, "the quiet check never announced the newer release");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // Without an install source the dialog offers only the page.
    assert!(h.query_by_label("Install now").is_none());
    h.get_by_label("Download");

    // Up to date, or a failed check: nothing appears.
    for answer in [Ok(installable(concat!("v", env!("CARGO_PKG_VERSION")))), Err("couldn't reach GitHub")] {
        let mut h = quiet_harness(answer, None);
        for _ in 0..40 {
            h.run_steps(2);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            h.query_by_label_contains("is up to date").is_none()
                && h.query_by_label_contains("Couldn't check").is_none()
                && h.query_by_label_contains("is available").is_none()
        );
    }
}

#[test]
fn install_now_runs_the_installer_source_and_reports_it() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = calls.clone();
    let install: InstallSource = Arc::new(move |r: &Release| {
        assert!(r.installer.is_some());
        counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    });
    let mut h = quiet_harness(Ok(installable("v99.0.0")), Some(install));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while h.query_by_label("Install now").is_none() {
        h.run_steps(2);
        assert!(std::time::Instant::now() < deadline, "the install button never appeared");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.get_by_label("Install now").click();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while h.query_by_label_contains("PdfCraft is updated").is_none() {
        h.run_steps(2);
        assert!(std::time::Instant::now() < deadline, "the installer's start was never reported");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn a_failed_installation_says_why_and_can_be_retried() {
    let install: InstallSource =
        Arc::new(|_: &Release| Err("the downloaded installer doesn't match its published checksum, so it was not run".into()));
    let mut h = quiet_harness(Ok(installable("v99.0.0")), Some(install));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while h.query_by_label("Install now").is_none() {
        h.run_steps(2);
        assert!(std::time::Instant::now() < deadline, "the install button never appeared");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.get_by_label("Install now").click();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while h.query_by_label_contains("Couldn't install the update").is_none() {
        h.run_steps(2);
        assert!(std::time::Instant::now() < deadline, "the failure was never reported");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.get_by_label_contains("doesn't match its published checksum");
    h.get_by_label("Install now");
}

/// Finding a newer release only announces it: nothing is downloaded or installed until the user
/// presses Install now.
#[test]
fn a_newer_release_is_never_installed_without_a_click() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = calls.clone();
    let install: InstallSource = Arc::new(move |_: &Release| {
        counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    });
    let mut h = quiet_harness(Ok(installable("v99.0.0")), Some(install));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while h.query_by_label("Install now").is_none() {
        h.run_steps(2);
        assert!(std::time::Instant::now() < deadline, "the dialog never offered the installation");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // Keep the dialog on screen for a good while, with the pointer wherever it is.
    for _ in 0..80 {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0, "the installer ran without a click");
    h.get_by_label("Install now");
}

#[test]
fn after_the_update_restart_now_starts_the_new_version_and_closes_this_one() {
    let restarts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = restarts.clone();
    let restart: RestartSource = Arc::new(move || {
        counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    });
    let install: InstallSource = Arc::new(|_: &Release| Ok(()));
    let mut h = quiet_harness_with_restart(Ok(installable("v99.0.0")), Some(install), Some(restart));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while h.query_by_label("Install now").is_none() {
        h.run_steps(2);
        assert!(std::time::Instant::now() < deadline, "the install button never appeared");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(h.query_by_label("Restart now").is_none(), "nothing to restart before the update is installed");
    h.get_by_label("Install now").click();
    while h.query_by_label("Restart now").is_none() {
        h.run_steps(2);
        assert!(std::time::Instant::now() < deadline, "Restart now never appeared after the update");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.get_by_label_contains("PdfCraft is updated");
    assert!(h.query_by_label("Install now").is_none(), "an installed update is not offered again");
    assert_eq!(restarts.load(std::sync::atomic::Ordering::SeqCst), 0, "nothing restarts without a click");
    h.get_by_label("Restart now").click();
    h.run_steps(3);
    assert_eq!(restarts.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn a_restart_that_fails_says_so_and_leaves_the_app_open() {
    let restart: RestartSource = Arc::new(|| Err("no helper".into()));
    let install: InstallSource = Arc::new(|_: &Release| Ok(()));
    let mut h = quiet_harness_with_restart(Ok(installable("v99.0.0")), Some(install), Some(restart));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while h.query_by_label("Install now").is_none() {
        h.run_steps(2);
        assert!(std::time::Instant::now() < deadline, "the install button never appeared");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.get_by_label("Install now").click();
    while h.query_by_label("Restart now").is_none() {
        h.run_steps(2);
        assert!(std::time::Instant::now() < deadline, "Restart now never appeared");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.get_by_label("Restart now").click();
    h.run_steps(3);
    h.get_by_label_contains("couldn't restart (no helper)");
}
