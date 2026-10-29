//! Asks GitHub for the latest release of this fork (Help ▸ Check for updates, issue #28) and, on
//! Windows, downloads its installer, checks the checksum published beside it and runs it.

use std::time::Duration;

use pdfcraft_ui_egui::updates::{Installer, RELEASES_PAGE, Release};

const LATEST: &str = "https://api.github.com/repos/caaioh1/pdfcraft-rs/releases/latest";
/// Where GitHub serves this repository's release files; an installer from anywhere else is ignored.
const DOWNLOADS: &str = "https://github.com/caaioh1/pdfcraft-rs/releases/download/";
/// The installer's name ends like this (`pdfcraft-<version>-windows-x64.msi`).
const INSTALLER_SUFFIX: &str = "-windows-x64.msi";
/// The checksum list published with each release (`<sha256>  <file name>` per line).
const CHECKSUMS: &str = "SHA256SUMS.txt";
/// An installer or checksum list larger than this is refused.
#[cfg(windows)]
const MAX_INSTALLER: u64 = 512 << 20;
#[cfg(windows)]
const MAX_CHECKSUMS: u64 = 1 << 20;

/// An HTTPS client that trusts the operating system's certificate authorities.
fn agent(timeout: Duration) -> Result<ureq::Agent, String> {
    Ok(ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .tls_config(ureq::tls::TlsConfig::builder().root_certs(os_roots()?).build())
        .build()
        .new_agent())
}

/// The latest release. The answer is untrusted: its size is capped, only a page under
/// [`RELEASES_PAGE`] is ever offered for download (anything else falls back to that list), and an
/// installer is offered only when GitHub serves it from this repository's release downloads.
pub fn latest_release() -> Result<Release, String> {
    let agent = agent(Duration::from_secs(10))?;
    let mut response = agent
        .get(LATEST)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", concat!("PdfCraft/", env!("CARGO_PKG_VERSION")))
        // Read error statuses ourselves: a 403 from GitHub usually means its anonymous
        // rate limit ran out, and the retry time rides along in a response header.
        .config()
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|e| format!("couldn't reach GitHub ({e})"))?;
    let status = response.status().as_u16();
    if status == 403 {
        let reset = response.headers().get("x-ratelimit-reset").and_then(|v| v.to_str().ok());
        return Err(rate_limited(reset, now_unix()));
    }
    if !(200..300).contains(&status) {
        return Err(format!("couldn't reach GitHub (http status: {status})"));
    }
    let body = response.body_mut().with_config().limit(1 << 20).read_to_string().map_err(|e| format!("unreadable answer ({e})"))?;
    parse(&body)
}

/// The certificate authorities the operating system trusts.
fn os_roots() -> Result<ureq::tls::RootCerts, String> {
    let found = rustls_native_certs::load_native_certs();
    let certs: Vec<ureq::tls::Certificate<'static>> = found.certs.iter().map(|c| ureq::tls::Certificate::from_der(c.as_ref()).to_owned()).collect();
    if certs.is_empty() {
        return Err("no trusted certificates found on this system".into());
    }
    Ok(ureq::tls::RootCerts::new_with_certs(&certs))
}

/// The current Unix time in seconds (0 when the clock gives nothing usable).
fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// What to tell the user when GitHub answers 403: its anonymous quota (60 checks an hour,
/// shared by everyone behind the same address) ran out. `reset` is the `x-ratelimit-reset`
/// header, the Unix time the quota refills; `now` is the current Unix time. Anything
/// surprising (no header, not a number, already past) falls back to asking for patience.
fn rate_limited(reset: Option<&str>, now: u64) -> String {
    const FALLBACK: &str = "GitHub is limiting update checks for now (shared hourly quota); try again later";
    let wait = reset.and_then(|r| r.trim().parse::<u64>().ok()).and_then(|reset| reset.checked_sub(now)).filter(|&wait| wait > 0);
    match wait {
        Some(wait) => {
            let minutes = wait.div_ceil(60).max(1);
            let plural = if minutes == 1 { "" } else { "s" };
            format!("GitHub is limiting update checks for now (shared hourly quota); try again in about {minutes} minute{plural}")
        }
        None => FALLBACK.into(),
    }
}

fn parse(body: &str) -> Result<Release, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| format!("unreadable answer ({e})"))?;
    let version = v["tag_name"].as_str().filter(|t| !t.is_empty() && t.len() <= 64).ok_or("no release found")?.to_string();
    let url = v["html_url"]
        .as_str()
        .filter(|u| u.strip_prefix(RELEASES_PAGE).is_some_and(|rest| rest.starts_with('/') && !rest.contains(['?', '#', '\\'])))
        .unwrap_or(RELEASES_PAGE)
        .to_string();
    Ok(Release { version, url, installer: installer_in(&v) })
}

/// What Windows Installer's exit code means for the update: 0 is done, 3010 is done but asks for a
/// restart of Windows (the new files are in place; the update takes effect on the next start).
#[cfg_attr(not(windows), allow(dead_code))]
fn install_outcome(code: Option<i32>) -> Result<(), String> {
    match code {
        Some(0 | 3010) => Ok(()),
        // 1602: cancelled in the installer; 1223: the permission prompt was refused.
        Some(1602 | 1223) => Err("the installation was cancelled (permission was not given)".into()),
        Some(1925) => Err("installing needs administrator permission".into()),
        Some(1618) => Err("another installation is already running; try again when it finishes".into()),
        Some(c) => Err(format!("the installer stopped with code {c}")),
        None => Err("the installer was stopped before it finished".into()),
    }
}

/// The PowerShell command that runs the installer `msi` elevated and silent, and exits with the
/// installer's own exit code (1223 when the permission prompt is refused). `/qn` shows no
/// window; the quotes around the path are for msiexec's command line.
///
/// `all_users`: the running copy is installed for every user. Since 0.14 the installer starts a new
/// installation for the current user only and refuses to update a copy of the other kind (it
/// stops with 1603), so an update of an every-user copy says `ALLUSERS=1`.
#[cfg_attr(not(windows), allow(dead_code))]
fn elevated_install_command(msi: &str, all_users: bool) -> String {
    let msi = msi.replace('\'', "''");
    let scope = if all_users { ", 'ALLUSERS=1'" } else { "" };
    format!(
        "try {{ $p = Start-Process -FilePath 'msiexec.exe' -ArgumentList @('/i', '\"{msi}\"', '/qn', '/norestart'{scope}) -Verb RunAs -Wait -PassThru -ErrorAction Stop; exit $p.ExitCode }} catch {{ exit 1223 }}"
    )
}

/// Whether `exe` lies under a Program Files folder, where the installer puts a copy installed for
/// every user (a per-user copy lives under `%LOCALAPPDATA%\Programs`).
#[cfg_attr(not(windows), allow(dead_code))]
fn installed_for_all_users(exe: &std::path::Path) -> bool {
    ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
        .iter()
        .filter_map(std::env::var_os)
        .filter(|v| !v.is_empty())
        .any(|root| exe.starts_with(std::path::PathBuf::from(root)))
}

/// Deletes the folders earlier updates left in the temporary folder (all `pdfcraft-update-<pid>`
/// but `keep`). Best effort: a folder in use is simply left alone.
#[cfg_attr(not(windows), allow(dead_code))]
fn clear_old_downloads(keep: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let ours = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("pdfcraft-update-"));
        if ours && path != keep && path.is_dir() {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// Removes a folder when dropped, whether the update succeeded, failed or returned early.
#[cfg_attr(not(windows), allow(dead_code))]
struct RemoveOnDrop(std::path::PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The PowerShell command that starts `exe` once process `pid` has exited. It waits at most five
/// minutes and starts nothing if the old process is still there (the user kept it open), so a
/// cancelled close can't open a second window later.
#[cfg_attr(not(windows), allow(dead_code))]
fn restart_command(pid: u32, exe: &str) -> String {
    let exe = exe.replace('\'', "''");
    format!(
        "Wait-Process -Id {pid} -Timeout 300 -ErrorAction SilentlyContinue; if (-not (Get-Process -Id {pid} -ErrorAction SilentlyContinue)) {{ Start-Process -FilePath '{exe}' }}"
    )
}

/// Starts PdfCraft again as soon as this process exits, so the version just installed opens in
/// its place. The caller closes the window next.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn restart_app() -> Result<(), String> {
    #[cfg(not(windows))]
    {
        Err("restarting after an update is only available on Windows".into())
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        /// CREATE_NO_WINDOW: the helper shows no console.
        const NO_WINDOW: u32 = 0x0800_0000;
        let exe = std::env::current_exe().map_err(|e| format!("couldn't find this program ({e})"))?;
        let script = restart_command(std::process::id(), &exe.to_string_lossy());
        std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script])
            .creation_flags(NO_WINDOW)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("couldn't start the restart helper ({e})"))
    }
}

/// A file name that is safe to put in a temporary folder: no separators, no leading dot.
fn plain_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 128 && !name.starts_with('.') && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// The release's Windows installer and checksum list, when both are served from [`DOWNLOADS`].
fn installer_in(release: &serde_json::Value) -> Option<Installer> {
    let assets = release["assets"].as_array()?;
    let find = |wanted: &dyn Fn(&str) -> bool| {
        assets.iter().find_map(|a| {
            let name = a["name"].as_str().filter(|n| plain_name(n) && wanted(n))?;
            let url = a["browser_download_url"].as_str()?;
            let rest = url.strip_prefix(DOWNLOADS)?;
            (rest.ends_with(name) && !rest.contains(['?', '#', '\\', ' '])).then(|| (name.to_string(), url.to_string()))
        })
    };
    let (file_name, msi_url) = find(&|n| n.ends_with(INSTALLER_SUFFIX))?;
    let (_, sums_url) = find(&|n| n == CHECKSUMS)?;
    Some(Installer { file_name, msi_url, sums_url })
}

/// The checksum `sums` (the text of `SHA256SUMS.txt`) lists for `file`, lower-cased. Only the
/// Windows installer reads it, so elsewhere it exists for its tests alone.
#[cfg(any(windows, test))]
fn checksum_for(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, name) = line.split_once(char::is_whitespace)?;
        let name = name.trim().trim_start_matches('*');
        (name == file && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit())).then(|| hash.to_ascii_lowercase())
    })
}

#[cfg(windows)]
fn get(agent: &ureq::Agent, url: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    let response = agent
        .get(url)
        .header("User-Agent", concat!("PdfCraft/", env!("CARGO_PKG_VERSION")))
        .config()
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|e| format!("couldn't download the update ({e})"))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(format!("couldn't download the update (http status: {status})"));
    }
    Ok(response)
}

/// Downloads the release's installer, checks it against the published checksum and starts it. The
/// installer runs on its own (Windows asks for permission, since PdfCraft is installed for every
/// user); this returns as soon as it has started. Nothing is run unless the checksum matches.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn install_release(release: &Release) -> Result<(), String> {
    #[cfg(not(windows))]
    {
        let _ = release;
        Err("installing updates automatically is only available on Windows".into())
    }
    #[cfg(windows)]
    {
        use std::io::{Read, Write};
        use std::os::windows::process::CommandExt;

        use sha2::{Digest, Sha256};

        /// CREATE_NO_WINDOW: the helper shows no console.
        const NO_WINDOW: u32 = 0x0800_0000;

        let installer = release.installer.as_ref().ok_or("this release has no installer for Windows")?;
        let agent = agent(Duration::from_secs(900))?;
        let sums = get(&agent, &installer.sums_url)?
            .body_mut()
            .with_config()
            .limit(MAX_CHECKSUMS)
            .read_to_string()
            .map_err(|e| format!("unreadable checksum list ({e})"))?;
        let expected = checksum_for(&sums, &installer.file_name).ok_or("the release doesn't list a checksum for its installer")?;

        let dir = std::env::temp_dir().join(format!("pdfcraft-update-{}", std::process::id()));
        clear_old_downloads(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| format!("couldn't prepare a download folder ({e})"))?;
        // The installer is not needed once it has run (or failed): don't leave 40 MB behind.
        let _cleanup = RemoveOnDrop(dir.clone());
        let path = dir.join(&installer.file_name);
        let mut response = get(&agent, &installer.msi_url)?;
        let mut reader = response.body_mut().with_config().limit(MAX_INSTALLER).reader();
        let mut file = std::fs::File::create(&path).map_err(|e| format!("couldn't save the installer ({e})"))?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = reader.read(&mut buf).map_err(|e| format!("the download was interrupted ({e})"))?;
            if n == 0 {
                break;
            }
            let chunk = buf.get(..n).ok_or("the download returned more than it was asked for")?;
            hasher.update(chunk);
            file.write_all(chunk).map_err(|e| format!("couldn't save the installer ({e})"))?;
        }
        file.flush().map_err(|e| format!("couldn't save the installer ({e})"))?;
        drop(file);
        let actual: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if actual != expected {
            let _ = std::fs::remove_file(&path);
            return Err("the downloaded installer doesn't match its published checksum, so it was not run".into());
        }
        // No properties, so an update keeps the desktop shortcut and context-menu choices of the
        // installed version (packaging/windows). Windows asks for permission once (PdfCraft is
        // installed for every user); the installer itself then runs without a window, and this
        // app's own dialog shows that it is working.
        let all_users = std::env::current_exe().is_ok_and(|exe| installed_for_all_users(&exe));
        let elevated = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-Command",
                &elevated_install_command(&path.to_string_lossy(), all_users),
            ])
            .creation_flags(NO_WINDOW)
            .status();
        match elevated {
            Ok(status) => install_outcome(status.code()),
            // No PowerShell to ask with: let Windows Installer ask and show its own progress.
            Err(_) => {
                let status = std::process::Command::new("msiexec")
                    .arg("/i")
                    .arg(&path)
                    .args(["/passive", "/norestart"])
                    .args(all_users.then_some("ALLUSERS=1"))
                    .status()
                    .map_err(|e| format!("couldn't start the installer ({e})"))?;
                install_outcome(status.code())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_read_and_only_our_release_pages_are_offered() {
        let r = parse(r#"{"tag_name":"v0.2.0","html_url":"https://github.com/caaioh1/pdfcraft-rs/releases/tag/v0.2.0"}"#).unwrap();
        assert_eq!(
            r,
            Release { version: "v0.2.0".into(), url: "https://github.com/caaioh1/pdfcraft-rs/releases/tag/v0.2.0".into(), installer: None }
        );
        for elsewhere in ["https://example.com/pdfcraft.exe", "https://github.com/caaioh1/pdfcraft-rs/releases.evil/x", "javascript:alert(1)"] {
            let r = parse(&format!(r#"{{"tag_name":"v9.9.9","html_url":"{elsewhere}"}}"#)).unwrap();
            assert_eq!(r.url, RELEASES_PAGE, "{elsewhere}");
        }
        assert!(parse(r#"{"message":"Not Found"}"#).is_err());
        assert!(parse("<html>").is_err());
    }

    fn release_json(assets: &str) -> String {
        format!(r#"{{"tag_name":"v0.9.0","html_url":"https://github.com/caaioh1/pdfcraft-rs/releases/tag/v0.9.0","assets":[{assets}]}}"#)
    }

    fn asset(name: &str, url: &str) -> String {
        format!(r#"{{"name":"{name}","browser_download_url":"{url}"}}"#)
    }

    #[test]
    fn the_installer_is_offered_only_from_our_release_downloads() {
        let base = "https://github.com/caaioh1/pdfcraft-rs/releases/download/v0.9.0";
        let msi = "pdfcraft-0.9.0-windows-x64.msi";
        let good = [
            asset("pdfcraft-0.9.0-windows-x64-portable.zip", &format!("{base}/pdfcraft-0.9.0-windows-x64-portable.zip")),
            asset(msi, &format!("{base}/{msi}")),
            asset("SHA256SUMS.txt", &format!("{base}/SHA256SUMS.txt")),
        ]
        .join(",");
        let found = parse(&release_json(&good)).unwrap().installer.unwrap();
        assert_eq!(found, Installer { file_name: msi.into(), msi_url: format!("{base}/{msi}"), sums_url: format!("{base}/SHA256SUMS.txt") });

        // Without the checksum list there is nothing to verify against, so no installer is offered.
        assert!(parse(&release_json(&asset(msi, &format!("{base}/{msi}")))).unwrap().installer.is_none());
        // A look-alike host, a path outside this repository's downloads, a query, or a name that
        // isn't a plain file name are all refused.
        for url in [
            format!("https://evil.example/{msi}"),
            format!("https://github.com/storytold/pdfcraft/releases/download/v0.9.0/{msi}"),
            format!("https://github.com/caaioh1/pdfcraft-rs/releases/download.evil/{msi}"),
            format!("{base}/{msi}?x=1"),
            format!("http://github.com/caaioh1/pdfcraft-rs/releases/download/v0.9.0/{msi}"),
        ] {
            let json = release_json(&[asset(msi, &url), asset("SHA256SUMS.txt", &format!("{base}/SHA256SUMS.txt"))].join(","));
            assert!(parse(&json).unwrap().installer.is_none(), "{url}");
        }
        let sneaky = "..\\\\evil-windows-x64.msi";
        let json = release_json(&[asset(sneaky, &format!("{base}/{sneaky}")), asset("SHA256SUMS.txt", &format!("{base}/SHA256SUMS.txt"))].join(","));
        assert!(parse(&json).unwrap().installer.is_none());
    }

    #[test]
    fn installer_exit_codes_mean_done_cancelled_or_failed() {
        assert_eq!(install_outcome(Some(0)), Ok(()));
        assert_eq!(install_outcome(Some(3010)), Ok(()), "done, Windows asks for a restart");
        assert!(install_outcome(Some(1602)).unwrap_err().contains("cancelled"));
        assert!(install_outcome(Some(1618)).unwrap_err().contains("already running"));
        assert!(install_outcome(Some(1603)).unwrap_err().contains("1603"));
        assert!(install_outcome(None).is_err());
    }

    #[test]
    fn the_install_runs_elevated_and_silent_with_the_path_quoted() {
        let c = elevated_install_command(r"C:\Users\Ana Maria\AppData\Local\Temp\pdfcraft-update-1\pdfcraft-0.9.0-windows-x64.msi", false);
        assert!(c.contains("-Verb RunAs"), "{c}");
        assert!(c.contains("'/qn'") && c.contains("'/norestart'"), "{c}");
        assert!(c.contains("-Wait -PassThru"), "{c}");
        // msiexec gets the path in double quotes, so a folder with a space stays one argument.
        assert!(c.contains(r#"'"C:\Users\Ana Maria\AppData"#), "{c}");
        assert!(c.contains("exit $p.ExitCode") && c.contains("exit 1223"), "{c}");
        // An apostrophe in the user name can't end the PowerShell string early.
        let q = elevated_install_command(r"C:\Users\O'Neil\x.msi", false);
        assert!(q.contains("O''Neil"), "{q}");
        assert!(!c.contains("ALLUSERS"), "{c}");
    }

    /// 0.14's installer refuses to update an every-user copy unless told so (it stopped with 1603).
    #[test]
    fn an_update_of_an_every_user_copy_says_all_users() {
        let c = elevated_install_command(r"C:\x.msi", true);
        assert!(c.contains("'ALLUSERS=1'"), "{c}");
        let per_user = std::path::Path::new(r"C:\Users\Ana\AppData\Local\Programs\ArtCraft\PdfCraft\pdfcraft.exe");
        assert!(!installed_for_all_users(per_user));
        #[cfg(windows)]
        if let Some(root) = std::env::var_os("ProgramFiles").filter(|v| !v.is_empty()) {
            assert!(installed_for_all_users(&std::path::PathBuf::from(root).join("ArtCraft").join("PdfCraft").join("pdfcraft.exe")));
        }
    }

    #[test]
    fn a_refused_permission_prompt_counts_as_cancelled() {
        assert!(install_outcome(Some(1223)).unwrap_err().contains("cancelled"));
        assert!(install_outcome(Some(1925)).unwrap_err().contains("administrator"));
    }

    #[test]
    fn only_the_updaters_own_folders_are_cleared() {
        let base = std::env::temp_dir().join(format!("pdfcraft-clear-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        // A folder that is not the updater's must survive, whatever the temp folder holds.
        let other = base.join("keep-me");
        std::fs::create_dir_all(&other).unwrap();
        let ours = std::env::temp_dir().join(format!("pdfcraft-update-test-{}", std::process::id()));
        std::fs::create_dir_all(&ours).unwrap();
        std::fs::write(ours.join("x.msi"), b"old").unwrap();
        clear_old_downloads(&base.join("not-a-folder"));
        assert!(!ours.exists(), "an old update folder is removed");
        assert!(other.exists(), "an unrelated folder stays");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_restart_waits_for_this_process_and_quotes_the_path() {
        let c = restart_command(4242, r"C:\Program Files\PdfCraft\pdfcraft.exe");
        assert!(c.starts_with("Wait-Process -Id 4242 -Timeout 300"), "{c}");
        assert!(c.contains(r"Start-Process -FilePath 'C:\Program Files\PdfCraft\pdfcraft.exe'"), "{c}");
        // Starts nothing while the old process is still running.
        assert!(c.contains("if (-not (Get-Process -Id 4242"), "{c}");
        // A quote in the path can't end the string early.
        let q = restart_command(1, r"C:\Users\O'Neil\pdfcraft.exe");
        assert!(q.contains("O''Neil"), "{q}");
    }

    #[test]
    fn checksums_are_looked_up_by_file_name() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let sums = format!("{a}  pdfcraft-0.9.0-windows-x64.msi\n{b} *other.zip\nnot-a-hash  broken.msi\n");
        assert_eq!(checksum_for(&sums, "pdfcraft-0.9.0-windows-x64.msi"), Some(a));
        assert_eq!(checksum_for(&sums, "other.zip"), Some("b".repeat(64)));
        assert_eq!(checksum_for(&sums, "broken.msi"), None);
        assert_eq!(checksum_for(&sums, "missing.msi"), None);
        assert_eq!(checksum_for("", "x"), None);
    }

    #[test]
    fn rate_limit_messages_name_the_wait() {
        let limited = "GitHub is limiting update checks for now (shared hourly quota); try again in about 20 minutes";
        assert_eq!(rate_limited(Some("1800001200"), 1800000000), limited);
        // Rounds up: 61 seconds out reads as 2 minutes.
        assert_eq!(
            rate_limited(Some("1800000061"), 1800000000),
            "GitHub is limiting update checks for now (shared hourly quota); try again in about 2 minutes"
        );
        assert_eq!(
            rate_limited(Some("1800000060"), 1800000000),
            "GitHub is limiting update checks for now (shared hourly quota); try again in about 1 minute"
        );
        // No header, not a number, or already past: ask for patience instead of a time.
        let later = "GitHub is limiting update checks for now (shared hourly quota); try again later";
        for bad in [None, Some(""), Some("soon"), Some("1799999999"), Some("1800000000")] {
            assert_eq!(rate_limited(bad, 1800000000), later);
        }
    }

    /// Live: asks GitHub over TLS with the OS's roots (`cargo test -p pdfcraft -- --ignored`).
    #[test]
    #[ignore = "needs network access"]
    fn github_answers_with_the_latest_release() {
        let r = latest_release().unwrap();
        assert!(pdfcraft_ui_egui::updates::is_newer(&r.version, "0.0.0"), "{r:?}");
        assert!(r.url.starts_with(RELEASES_PAGE), "{r:?}");
    }
}
