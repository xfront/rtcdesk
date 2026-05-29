use std::env;
use std::fs;

fn main() {
    let out = env::var("CARGO_MANIFEST_DIR").unwrap();
    let triple = rustc_host_triple();
    let src = format!(
        "{}/../target/release/rtcdesk-service",
        env::var("CARGO_WORKSPACE_DIR").unwrap_or("..".into())
    );
    let dst_dir = format!("{}/host", out);
    fs::create_dir_all(&dst_dir).unwrap();
    let dst = format!("{}/rtcdesk-service-{}", dst_dir, triple);
    if std::path::Path::new(&src).exists() {
        fs::copy(src, &dst).unwrap();
    }
}

fn rustc_host_triple() -> String {
    let output = std::process::Command::new("rustc")
        .arg("-vV")
        .output()
        .unwrap();
    let s = String::from_utf8(output.stdout).unwrap();
    s.lines()
        .find(|l| l.starts_with("host:"))
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_string()
}