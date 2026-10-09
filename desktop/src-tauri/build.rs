fn main() {
    // 提交号编进程序：调试版的窗口标题和侧栏会显示它，一眼就能看出运行的是哪一次构建
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    println!("cargo:rustc-env=CLEARCLIP_COMMIT={}", commit());
    tauri_build::build()
}

/// 流水线里用 `GITHUB_SHA`，本机用 `git rev-parse`，都拿不到就写 unknown。
fn commit() -> String {
    let from_env = std::env::var("GITHUB_SHA").ok().map(|s| s.trim().chars().take(7).collect::<String>()).filter(|s| !s.is_empty());
    from_env
        .or_else(|| {
            let out = std::process::Command::new("git").args(["rev-parse", "--short=7", "HEAD"]).output().ok()?;
            let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
            (out.status.success() && !s.is_empty()).then_some(s)
        })
        .unwrap_or_else(|| "unknown".into())
}
