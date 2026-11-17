use std::path::PathBuf;
use std::process::Command;

fn main() {
    embuild::espidf::sysenv::output();

    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    println!("cargo:rerun-if-changed=res/style/gamebub");

    slint_build::compile_with_config(
        "res/ui/main.slint",
        slint_build::CompilerConfiguration::new()
            .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer)
            .with_style("gamebub".into())
            .with_include_paths(vec![
                manifest_dir.join("res/style"),
                manifest_dir.join("res/style/gamebub"),
            ]),
    )
    .unwrap();

    // Get git commit hash
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git command failed");
    let commit_hash = str::from_utf8(&output.stdout)
        .expect("git output invalid utf-8")
        .trim();
    println!("cargo:rustc-env=GIT_COMMIT={}", commit_hash);

    println!("cargo:rustc-link-arg=-Wl,--wrap=esp_panic_handler");
}
