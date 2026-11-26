use std::path::PathBuf;
use std::process::Command;

fn main() {
    tauri_build::build();

    // Roadmap 124: a build stamp baked into the binary so About can name the exact commit the
    // user is running. A source archive without .git reports `unknown` instead of guessing; the
    // renderer version and this stamp are the only release facts the dialog trusts.
    let commit = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| {
            let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
            (!hash.is_empty()).then_some(hash)
        })
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=RIGEL_BUILD_COMMIT={commit}");
    // Worktrees keep their HEAD outside the checkout, so ask git for the real path instead of
    // guessing one relative to this crate.
    if let Ok(output) = Command::new("git")
        .args(["rev-parse", "--absolute-git-dir"])
        .output()
        && output.status.success()
    {
        let head = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()).join("HEAD");
        println!("cargo:rerun-if-changed={}", head.display());
    }
}
