//! Spreading a new version across the devices in a room.
//!
//! There is no update server. When one machine is given a newer build, it
//! says so in its announcements; the others pull the app from it over the
//! same encrypted channel they use for everything else, check that what
//! arrived is exactly what was advertised, and swap it in.

use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Updates replace the running app, so when one goes wrong there has to be a
/// record of why. Appends to update.log next to the settings file.
pub fn log(msg: impl AsRef<str>) {
    let msg = msg.as_ref();
    eprintln!("[netpaste update] {}", msg);
    let path = crate::config::config_dir().join("update.log");
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        use std::io::Write;
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "{} v{} {}", secs, VERSION, msg);
    }
}

/// Builds that can use each other's updates. macOS ships one universal
/// bundle, so both kinds of Mac share a key; elsewhere the architecture
/// matters.
pub fn platform_key() -> String {
    if cfg!(target_os = "macos") {
        "macos-universal".into()
    } else {
        format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
    }
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 128 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{:02x}", b)).collect())
}

pub fn self_exe() -> std::io::Result<PathBuf> {
    std::env::current_exe()
}

pub fn self_exe_sha() -> String {
    self_exe().and_then(|p| sha256_file(&p)).unwrap_or_default()
}

/// The .app this executable lives in, if any.
pub fn bundle_root() -> Option<PathBuf> {
    let exe = self_exe().ok()?;
    // …/NetPaste.app/Contents/MacOS/NetPaste
    let root = exe.parent()?.parent()?.parent()?;
    (root.extension().map(|e| e == "app").unwrap_or(false)).then(|| root.to_path_buf())
}

/// "1.2.3" -> (1, 2, 3). Anything unparseable sorts lowest.
pub fn parse_version(v: &str) -> (u32, u32, u32) {
    let mut it = v.trim().split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0))
}

pub fn is_newer(theirs: &str, mine: &str) -> bool {
    parse_version(theirs) > parse_version(mine)
}

pub fn staging_dir() -> PathBuf {
    std::env::temp_dir().join("netpaste-update")
}

/// Wrap this app up so a peer can download it: a zipped .app on macOS, the
/// bare executable everywhere else. Returns what to send and the fingerprint
/// of the executable inside it.
pub fn package_self() -> std::io::Result<(PathBuf, String)> {
    let dir = staging_dir();
    std::fs::create_dir_all(&dir)?;
    let exe = self_exe()?;
    let payload = match bundle_root() {
        Some(app) => {
            let out = dir.join("NetPaste-offer.zip");
            let _ = std::fs::remove_file(&out);
            let status = Command::new("ditto")
                .args(["-c", "-k", "--keepParent"])
                .arg(&app)
                .arg(&out)
                .status()?;
            if !status.success() {
                return Err(other("could not package the app bundle"));
            }
            out
        }
        None => exe.clone(),
    };
    Ok((payload, sha256_file(&exe)?))
}

/// Unpack what a peer sent, make sure it is genuinely the advertised build
/// and that it can run here, then put it in place and restart.
pub fn apply(downloaded: &Path, expected_exe_sha: &str) -> std::io::Result<()> {
    let dir = staging_dir().join("staged");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;

    let (new_exe, new_payload): (PathBuf, PathBuf) = if bundle_root().is_some() {
        let status = Command::new("ditto").args(["-x", "-k"]).arg(downloaded).arg(&dir).status()?;
        if !status.success() {
            return Err(other("the update archive could not be unpacked"));
        }
        let app = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.extension().map(|e| e == "app").unwrap_or(false))
            .ok_or_else(|| other("no app bundle inside the update"))?;
        (app.join("Contents/MacOS/NetPaste"), app)
    } else {
        let exe = dir.join(self_exe()?.file_name().unwrap_or_else(|| "NetPaste".as_ref()));
        std::fs::copy(downloaded, &exe)?;
        (exe.clone(), exe)
    };

    log(format!("unpacked to {}", new_exe.display()));

    // 1. It must be exactly the build that was advertised.
    let got = sha256_file(&new_exe)?;
    if got != expected_exe_sha {
        log(format!("fingerprint mismatch: got {} wanted {}", got, expected_exe_sha));
        return Err(other("the update did not match its fingerprint - ignored"));
    }

    // 2. It must be able to run on this machine.
    #[cfg(target_os = "macos")]
    {
        let out = Command::new("lipo").arg("-archs").arg(&new_exe).output()?;
        let archs = String::from_utf8_lossy(&out.stdout).to_string();
        // lipo speaks Apple's names, not Rust's: aarch64 is "arm64" there.
        let wanted = match std::env::consts::ARCH {
            "aarch64" => "arm64",
            other => other,
        };
        if !archs.split_whitespace().any(|a| a == wanted) {
            log(format!("update has [{}], this Mac needs {}", archs.trim(), wanted));
            return Err(other("the update does not include this Mac's architecture"));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new_exe, std::fs::Permissions::from_mode(0o755))?;
    }

    // 3. Swap, keeping the old copy until the new one has started.
    let target = bundle_root().map(Ok).unwrap_or_else(self_exe)?;
    let backup = target.with_extension("netpaste-old");
    log(format!("swapping {} <- {}", target.display(), new_payload.display()));
    let _ = std::fs::remove_dir_all(&backup);
    let _ = std::fs::remove_file(&backup);
    if let Err(e) = std::fs::rename(&target, &backup) {
        log(format!("could not move the old app aside: {}", e));
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&new_payload, &target) {
        log(format!("could not move the new app in: {} - rolling back", e));
        let _ = std::fs::rename(&backup, &target); // put the old one back
        return Err(e);
    }

    log("swapped in, restarting");
    relaunch(&target)
}

/// Clear the previous version left behind by an earlier update.
/// Clear what an update leaves behind: the previous version, and the staged
/// copies in the temp directory. Both are several megabytes, so they are
/// cleared on every start rather than left to pile up.
pub fn clean_old_version() {
    let Ok(exe) = self_exe() else { return };
    let target = bundle_root().unwrap_or(exe);
    let backup = target.with_extension("netpaste-old");
    let _ = std::fs::remove_dir_all(&backup);
    let _ = std::fs::remove_file(&backup);
    let _ = std::fs::remove_dir_all(staging_dir());
}

fn relaunch(target: &Path) -> ! {
    #[cfg(target_os = "macos")]
    let spawned = if target.extension().map(|e| e == "app").unwrap_or(false) {
        Command::new("open").arg("-n").arg(target).spawn()
    } else {
        Command::new(target).spawn()
    };
    #[cfg(not(target_os = "macos"))]
    let spawned = Command::new(target).spawn();

    let _ = spawned;
    std::process::exit(0);
}

fn other(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, msg)
}
