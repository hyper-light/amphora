use std::path::Path;
use std::process::Command;

fn git_output(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

fn main() {
    let git_dir = Path::new("../.git");
    let head_path = git_dir.join("HEAD");
    if head_path.exists() {
        println!("cargo:rerun-if-changed=../.git/HEAD");
        if let Ok(head) = std::fs::read_to_string(&head_path)
            && let Some(reference) = head.trim().strip_prefix("ref: ")
        {
            println!("cargo:rerun-if-changed=../.git/{reference}");
        }
    }
    if git_dir.join("index").exists() {
        println!("cargo:rerun-if-changed=../.git/index");
    }

    let commit = git_output(&["rev-parse", "HEAD"])
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_owned());
    let dirty = git_output(&["status", "--porcelain"])
        .map(|s| s.lines().count())
        .unwrap_or(0);

    println!("cargo:rustc-env=AMPHORA_BENCH_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=AMPHORA_BENCH_GIT_DIRTY={dirty}");
}
