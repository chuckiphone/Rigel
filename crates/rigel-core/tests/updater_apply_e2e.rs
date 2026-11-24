//! A real deferred replace: a throwaway copy of an executable is replaced by a downloaded, signed
//! asset while it is running, and the new build is actually started.
//!
//! The feed is a loopback HTTP server with a signed manifest. The "app" is a compiled stub copied
//! into a temp directory. The helper is the real `rigel-apply-update` binary running the real
//! `rigel_core::update::apply` code, so nothing about the swap or the relaunch is mocked.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ed25519_dalek::{Signer, SigningKey};
use rigel_core::update::{
    HELPER_FLAG, PUBKEY_FILE, UpdateManifest, UpdateSettings, check_latest_at, save_settings,
    stage_binary,
};
use sha2::{Digest, Sha256};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// A stub executable. It records its start and, when asked, answers the helper's launch-health
/// check. The old build stays alive so the helper has something to wait for; the replacement can
/// either stay healthy through the window or exit immediately to exercise rollback.
fn build_stub(
    dir: &Path,
    name: &str,
    marker: &Path,
    sleep_secs: Option<u64>,
    healthy: bool,
) -> PathBuf {
    let source = dir.join(format!("{name}.rs"));
    let exe = dir.join(if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    });
    let marker = format!("{:?}", marker.to_string_lossy());
    let body = sleep_secs
        .map(|secs| format!("std::thread::sleep(std::time::Duration::from_secs({secs}));"))
        .unwrap_or_default();
    let health = if healthy {
        r#"    if let Ok(path) = std::env::var("RIGEL_UPDATE_HEALTH") {
        std::fs::write(path, format!("{} {}", std::process::id(), guard)).unwrap();
    }
"#
    } else {
        ""
    };
    fs::write(
        &source,
        format!(
            r#"fn main() {{
    let args = std::env::args().skip(1).collect::<Vec<_>>().join(",");
    let cwd = std::env::current_dir().unwrap().display().to_string();
    let guard = std::env::var("RIGEL_UPDATED_TO").unwrap_or_default();
    std::fs::write({marker}, format!("{{}}|{{}}|{{}}|{{}}", {name:?}, args, cwd, guard)).unwrap();
{health}
    {body}
}}
"#
        ),
    )
    .unwrap();
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = Command::new(rustc)
        .args(["--edition", "2021", "-o"])
        .arg(&exe)
        .arg(&source)
        .output()
        .expect("rustc builds the stub");
    assert!(
        output.status.success(),
        "rustc failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    exe
}

fn manifest(version: &str, asset_url: &str, asset_sha: &str, key: &SigningKey) -> UpdateManifest {
    let mut manifest = UpdateManifest {
        version: version.to_string(),
        url: asset_url.to_string(),
        sha256: asset_sha.to_string(),
        signature: String::new(),
        notes: None,
        published_at: None,
        msi_url: None,
        msi_sha256: None,
        bin_url: Some(asset_url.to_string()),
        bin_sha256: Some(asset_sha.to_string()),
        bin_signature: None,
        bin_target: Some(rigel_core::update::platform_tag()),
    };
    manifest.signature = hex(&key.sign(&manifest.signed_bytes()).to_bytes());
    let payload = manifest.binary_signed_bytes().unwrap();
    manifest.bin_signature = Some(hex(&key.sign(&payload).to_bytes()));
    manifest
}

/// Serve the signed manifest and its asset on an already-bound listener. Binding first is what
/// lets the manifest name the port it is served on, which is part of the signed payload.
fn serve(
    listener: TcpListener,
    manifest: String,
    asset: Vec<u8>,
    asset_path: String,
) -> Arc<Mutex<Vec<String>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    std::thread::spawn(move || {
        for socket in listener.incoming() {
            let Ok(mut socket) = socket else { break };
            let mut request = [0u8; 4096];
            let read = socket.read(&mut request).unwrap_or(0);
            let request = String::from_utf8_lossy(&request[..read]);
            let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
            recorded.lock().unwrap().push(path.clone());
            let (status, body) = if path == "/latest.json" {
                ("200 OK", manifest.as_bytes().to_vec())
            } else if path == asset_path {
                ("200 OK", asset.clone())
            } else {
                ("404 Not Found", Vec::new())
            };
            let header = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = socket.write_all(header.as_bytes());
            let _ = socket.write_all(&body);
        }
    });
    seen
}

fn bind() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

fn data_dir(key: &SigningKey, feed: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join(PUBKEY_FILE),
        hex(&key.verifying_key().to_bytes()),
    )
    .unwrap();
    save_settings(
        dir.path(),
        &UpdateSettings {
            enabled: true,
            manifest_url: Some(feed.to_string()),
            auto_install: true,
        },
    )
    .unwrap();
    dir
}

fn wait_for(path: &Path, limit: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if path.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    path.exists()
}

fn stub_name() -> &'static str {
    if cfg!(windows) {
        "rigel-app.exe"
    } else {
        "rigel-app"
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_newer_feed_replaces_the_running_binary_and_starts_the_new_build() {
    let work = tempfile::tempdir().unwrap();
    let app_dir = work.path().join("app");
    let run_dir = work.path().join("run");
    fs::create_dir_all(&app_dir).unwrap();
    fs::create_dir_all(&run_dir).unwrap();

    let old_marker = work.path().join("old.txt");
    let new_marker = work.path().join("new.txt");
    let old_stub = build_stub(work.path(), "old", &old_marker, Some(120), false);
    let new_stub = build_stub(work.path(), "new", &new_marker, Some(5), true);

    let target = app_dir.join(stub_name());
    fs::copy(&old_stub, &target).unwrap();
    let old_bytes = fs::read(&target).unwrap();
    let new_bytes = fs::read(&new_stub).unwrap();
    assert_ne!(old_bytes, new_bytes);

    let key = SigningKey::from_bytes(&[31u8; 32]);
    let (listener, port) = bind();
    let asset_url = format!("http://127.0.0.1:{port}/rigel-app.exe");
    let signed = manifest("9.9.9", &asset_url, &sha(&new_bytes), &key);
    let seen = serve(
        listener,
        serde_json::to_string(&signed).unwrap(),
        new_bytes.clone(),
        "/rigel-app.exe".to_string(),
    );
    let feed = format!("http://127.0.0.1:{port}/latest.json");
    let data = data_dir(&key, &feed);

    let args = vec!["--profile".to_string(), "e2e".to_string()];
    let mut child = Command::new(&target)
        .args(&args)
        .current_dir(&run_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the old stub starts");
    let pid = child.id();
    assert!(
        wait_for(&old_marker, Duration::from_secs(10)),
        "the old build recorded its start"
    );

    let check = check_latest_at(data.path(), "0.3.0", &feed)
        .await
        .expect("the signed 9.9.9 manifest verifies");
    assert!(check.available);
    assert_eq!(check.bin_url.as_deref(), Some(asset_url.as_str()));

    let staged = stage_binary(
        data.path(),
        &check,
        &target,
        pid,
        args.clone(),
        Some(run_dir.clone()),
    )
    .await
    .expect("the signed binary downloads")
    .expect("a newer feed stages a binary");

    let mut helper = Command::new(env!("CARGO_BIN_EXE_rigel-apply-update"))
        .arg(HELPER_FLAG)
        .arg(&staged.plan)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the helper starts");
    assert!(
        wait_for(
            &data.path().join("updates/apply.log"),
            Duration::from_secs(10)
        ),
        "the helper records its wait"
    );
    std::thread::sleep(Duration::from_millis(300));
    child.kill().expect("the old build is stopped");
    let _ = child.wait();

    assert!(
        wait_for(&new_marker, Duration::from_secs(20)),
        "the new build is actually started"
    );
    let marker = fs::read_to_string(&new_marker).unwrap();
    assert!(
        marker.starts_with("new|--profile,e2e|"),
        "args kept: {marker}"
    );
    assert!(
        marker.contains(run_dir.to_string_lossy().as_ref()),
        "working directory kept: {marker}"
    );
    assert!(
        marker.ends_with("|9.9.9"),
        "the relaunch carries the applied version: {marker}"
    );
    let status = helper
        .wait()
        .expect("the helper finishes the health window");
    assert!(status.success(), "a healthy relaunch completes");
    assert_eq!(
        fs::read(&target).unwrap(),
        new_bytes,
        "the binary is replaced"
    );
    assert!(!PathBuf::from(format!("{}.old", target.display())).exists());
    let log = fs::read_to_string(data.path().join("updates/apply.log")).unwrap();
    assert!(log.contains("waiting for pid"), "log: {log}");
    assert!(log.contains("swapped"), "log: {log}");
    assert!(log.contains("launched healthy"), "log: {log}");
    assert!(log.contains("started"), "log: {log}");
    println!("relaunched marker: {marker}");
    println!("apply log:\n{log}");

    let requests = seen.lock().unwrap().clone();
    println!("feed requests: {requests:?}");
    assert_eq!(
        requests,
        vec!["/latest.json".to_string(), "/rigel-app.exe".to_string()],
        "only the manifest and the one asset were fetched"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_build_that_exits_before_it_is_healthy_is_rolled_back() {
    let work = tempfile::tempdir().unwrap();
    let app_dir = work.path().join("app");
    let run_dir = work.path().join("run");
    fs::create_dir_all(&app_dir).unwrap();
    fs::create_dir_all(&run_dir).unwrap();

    let old_marker = work.path().join("old.txt");
    let new_marker = work.path().join("new.txt");
    let old_stub = build_stub(work.path(), "old", &old_marker, Some(120), false);
    let new_stub = build_stub(work.path(), "new", &new_marker, None, false);

    let target = app_dir.join(stub_name());
    fs::copy(&old_stub, &target).unwrap();
    let old_bytes = fs::read(&target).unwrap();
    let new_bytes = fs::read(&new_stub).unwrap();

    let key = SigningKey::from_bytes(&[33u8; 32]);
    let (listener, port) = bind();
    let asset_url = format!("http://127.0.0.1:{port}/rigel-app.exe");
    let signed = manifest("9.9.9", &asset_url, &sha(&new_bytes), &key);
    let _seen = serve(
        listener,
        serde_json::to_string(&signed).unwrap(),
        new_bytes.clone(),
        "/rigel-app.exe".to_string(),
    );
    let feed = format!("http://127.0.0.1:{port}/latest.json");
    let data = data_dir(&key, &feed);

    let args = vec!["--profile".to_string(), "rollback".to_string()];
    let mut child = Command::new(&target)
        .args(&args)
        .current_dir(&run_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the old stub starts");
    let pid = child.id();
    assert!(
        wait_for(&old_marker, Duration::from_secs(10)),
        "the old build recorded its start"
    );
    fs::remove_file(&old_marker).unwrap();

    let check = check_latest_at(data.path(), "0.3.0", &feed)
        .await
        .expect("the signed 9.9.9 manifest verifies");
    let staged = stage_binary(
        data.path(),
        &check,
        &target,
        pid,
        args.clone(),
        Some(run_dir.clone()),
    )
    .await
    .expect("the signed binary downloads")
    .expect("a newer feed stages a binary");

    let mut helper = Command::new(env!("CARGO_BIN_EXE_rigel-apply-update"))
        .arg(HELPER_FLAG)
        .arg(&staged.plan)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the helper starts");
    assert!(
        wait_for(
            &data.path().join("updates/apply.log"),
            Duration::from_secs(10)
        ),
        "the helper records its wait"
    );
    std::thread::sleep(Duration::from_millis(300));
    child.kill().expect("the old build is stopped");
    let _ = child.wait();

    assert!(
        wait_for(&old_marker, Duration::from_secs(20)),
        "the previous build is restarted"
    );
    assert!(
        new_marker.exists(),
        "the replacement was launched before it was rolled back"
    );
    assert_eq!(
        fs::read(&target).unwrap(),
        old_bytes,
        "the old binary is restored"
    );
    assert!(!PathBuf::from(format!("{}.old", target.display())).exists());
    let status = helper.wait().expect("the helper finishes");
    assert!(status.success(), "rollback is a completed outcome");
    let log = fs::read_to_string(data.path().join("updates/apply.log")).unwrap();
    assert!(log.contains("rolled back"), "log: {log}");
    assert!(log.contains("restarted previous build"), "log: {log}");
    assert!(
        rigel_core::update::apply::rollback_blocked(data.path(), "9.9.9"),
        "the failed version is blocked from an immediate retry"
    );
    assert!(
        !rigel_core::update::apply::rollback_blocked(data.path(), "9.9.10"),
        "a different release is not blocked"
    );
    println!("rollback apply log:\n{log}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_current_feed_downloads_nothing_and_leaves_the_binary_alone() {
    let work = tempfile::tempdir().unwrap();
    let target = work.path().join(stub_name());
    let marker = work.path().join("marker.txt");
    let stub = build_stub(work.path(), "app", &marker, None, false);
    fs::copy(&stub, &target).unwrap();
    let before = fs::read(&target).unwrap();

    let key = SigningKey::from_bytes(&[32u8; 32]);
    let asset = fs::read(&stub).unwrap();
    let (listener, port) = bind();
    let asset_url = format!("http://127.0.0.1:{port}/rigel-app.exe");
    let signed = manifest("0.3.0", &asset_url, &sha(&asset), &key);
    let seen = serve(
        listener,
        serde_json::to_string(&signed).unwrap(),
        asset,
        "/rigel-app.exe".to_string(),
    );
    let feed = format!("http://127.0.0.1:{port}/latest.json");
    let data = data_dir(&key, &feed);

    let check = check_latest_at(data.path(), "0.3.0", &feed)
        .await
        .expect("the current manifest still verifies");
    assert!(!check.available);
    assert!(
        stage_binary(
            data.path(),
            &check,
            &target,
            std::process::id(),
            Vec::new(),
            None
        )
        .await
        .expect("a current build stages nothing")
        .is_none(),
        "there is no artifact for a current build"
    );
    assert_eq!(
        fs::read(&target).unwrap(),
        before,
        "the binary is untouched"
    );
    assert!(!data.path().join("updates").exists());
    assert!(!marker.exists(), "nothing was started");
    let requests = seen.lock().unwrap().clone();
    println!("current build requests: {requests:?} (no download, binary intact)");
    assert_eq!(
        requests,
        vec!["/latest.json".to_string()],
        "a current build never fetches its asset"
    );
}
