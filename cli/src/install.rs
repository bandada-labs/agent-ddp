//! Installing an APK: not again when the device already holds those bytes, and with adb's own words when Android
//! turns it down.
use crate::state::{adb_path, capture, failure, install_refused_by_phone, which, Failure, Output, Result};
use crate::{fail, say};
use std::path::Path;
use std::time::Instant;

/// The file's SHA-256, from the system's own tool. None without one (Windows): the APK is then installed every time.
fn sha256(file: &str) -> Option<String> {
    // ponytail: sha256sum or shasum, no crate; a crate when Windows needs the skip too.
    let hashed = match which("sha256sum") {
        Some(tool) => capture(&tool, &[file]),
        None => capture(&which("shasum")?, &["-a", "256", file]),
    };
    let hash = hashed.stdout.split_whitespace().next()?;
    (hashed.code == 0 && hash.len() == 64).then(|| hash.to_string())
}

/// The installed apps with their APK paths, then the SHA-256 of each of those APKs, in one round trip.
const INSTALLED_HASHES: &str = "l=$(pm list packages -f -3); echo \"$l\"; \
    sha256sum $(echo \"$l\" | sed -e 's/^package://' -e 's/=[^=]*$//') </dev/null";

/// The package whose installed APK has this hash, in what `INSTALLED_HASHES` printed.
fn package_with_hash(listing: &str, hash: &str) -> Option<String> {
    let installed_apk = listing.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        (words.next() == Some(hash)).then(|| words.next()).flatten()
    })?;
    listing.lines().find_map(|line| {
        let (apk, package) = line.strip_prefix("package:")?.rsplit_once('=')?;
        (apk == installed_apk).then(|| package.trim().to_string())
    })
}

/// Installs the APK on the device. False when the device already has an app installed from these same bytes: then
/// nothing is sent, and the app's data stays as `install -r` would leave it.
pub fn apk(device: &str, apk: &str) -> Result<bool> {
    let started = Instant::now();
    let seconds = || format!("{:.1}", started.elapsed().as_secs_f64());
    if !Path::new(apk).is_file() {
        fail!(format!("no APK at {apk}"), "file_not_found");
    }
    let adb = adb_path()?;
    let on_device = sha256(apk)
        .and_then(|hash| package_with_hash(&capture(&adb, &["-s", device, "shell", INSTALLED_HASHES]).stdout, &hash));
    if let Some(package) = on_device {
        say!("{apk} is on {device} already ({package}, the same bytes): not installed again, {}s", seconds());
        return Ok(false);
    }
    // -t: a build marked test-only (what Android Studio runs) installs too.
    let installed = capture(&adb, &["-s", device, "install", "-r", "-t", apk]);
    if let Some(refused) = install_refused_by_phone(&installed, apk) {
        return Err(refused);
    }
    if installed.code != 0 || installed.stdout.contains("Failure") {
        // On an emulator ddp started, its data is disposable: an app signed with another key (one baked into the
        // base's snapshot by an earlier build) goes, and the install runs once more. Any other device keeps its app.
        let Some(package) = signed_with_another_key(&installed).filter(|_| crate::up::can_restart(device)) else {
            return Err(turned_down(device, apk, &installed));
        };
        capture(&adb, &["-s", device, "uninstall", &package]);
        let again = capture(&adb, &["-s", device, "install", "-r", "-t", apk]);
        if again.code != 0 || again.stdout.contains("Failure") {
            return Err(turned_down(device, apk, &again));
        }
        say!("installed {apk} in {}s: the {package} on {device} was signed with another key, and the emulator is ddp's own, so it was uninstalled first", seconds());
        return Ok(true);
    }
    say!("installed {apk} in {}s", seconds());
    Ok(true)
}

/// The package an install was turned down for because the one on the device was signed with another key.
/// adb: "INSTALL_FAILED_UPDATE_INCOMPATIBLE: Existing package com.example signatures do not match newer version".
fn signed_with_another_key(adb_said: &Output) -> Option<String> {
    let words = format!("{}\n{}", adb_said.stderr, adb_said.stdout);
    if !words.contains("INSTALL_FAILED_UPDATE_INCOMPATIBLE") {
        return None;
    }
    words.split("Existing package ").nth(1)?.split_whitespace().next().map(String::from)
}

/// An install Android turned down, in adb's own words: adb writes the reason to stderr, and stdout holds only
/// "Performing Streamed Install". For an app signed with another key than the installed one, also what that means.
fn turned_down(device: &str, apk: &str, adb_said: &Output) -> Failure {
    let words: Vec<&str> =
        [adb_said.stderr.trim(), adb_said.stdout.trim()].into_iter().filter(|said| !said.is_empty()).collect();
    let words = words.join("\n");
    let message = format!("{apk} was not installed on {device}. adb said: {words}");
    match signed_with_another_key(adb_said) {
        Some(package) => failure(
            format!("{message}\nThe {package} on the device was signed with another key, and Android updates an app only with the key it was installed with."),
            "signing_key_differs",
            Some(&format!("adb -s {device} uninstall {package} (its data on the device goes too), then the same command")),
        ),
        None => failure(message, "install_failed", None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = "package:/data/app/~~Qx3==/dev.agentddp.sample-a1b2==/base.apk=dev.agentddp.sample\n\
        package:/data/app/~~Zz9==/dev.agentddp.sample.test-c3d4==/base.apk=dev.agentddp.sample.test\n\
        aaaa  /data/app/~~Qx3==/dev.agentddp.sample-a1b2==/base.apk\n\
        bbbb  /data/app/~~Zz9==/dev.agentddp.sample.test-c3d4==/base.apk\n";

    #[test]
    fn finds_the_app_installed_from_the_same_bytes() {
        assert_eq!(package_with_hash(LISTING, "bbbb").as_deref(), Some("dev.agentddp.sample.test"));
        assert_eq!(package_with_hash(LISTING, "cccc"), None);
        assert_eq!(package_with_hash("", "aaaa"), None);
    }

    #[test]
    fn another_signing_key_is_said_in_adbs_words_and_explained() {
        let said = Output {
            code: 1,
            stdout: "Performing Streamed Install\n".into(),
            stderr: "adb: failed to install app.apk: Failure [INSTALL_FAILED_UPDATE_INCOMPATIBLE: Existing package \
                     com.example signatures do not match newer version; ignoring!]\n"
                .into(),
        };
        let failure = turned_down("emulator-5560", "app.apk", &said);
        assert_eq!(failure.code, "signing_key_differs");
        assert!(failure.message.contains("INSTALL_FAILED_UPDATE_INCOMPATIBLE: Existing package com.example"));
        assert_eq!(
            failure.next.as_deref(),
            Some("adb -s emulator-5560 uninstall com.example (its data on the device goes too), then the same command")
        );
        let other =
            Output { code: 1, stdout: String::new(), stderr: "adb: failed: Failure [INSTALL_FAILED_OLDER_SDK]".into() };
        assert_eq!(turned_down("emulator-5560", "app.apk", &other).code, "install_failed");
    }
}
