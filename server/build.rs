fn main() {
    // rust-embed requires the frontend folder to exist at compile time,
    // but web-ui/dist is a build artifact (gitignored). Ensure it exists
    // so a fresh checkout compiles; `just web-build` fills it for real.
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../web-ui/dist");
    if !dist.exists() {
        let _ = std::fs::create_dir_all(&dist);
        let _ = std::fs::write(
            dist.join("index.html"),
            "<!doctype html><title>weave-server</title>build the frontend with `just web-build`",
        );
    }
    println!("cargo::rerun-if-changed=../web-ui/dist");
}
