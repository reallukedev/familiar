// The portal is embedded from web-dist. A server-only build (no `npm run build` yet)
// still compiles and serves a short page explaining how to build the portal.
fn main() {
    let dist = std::path::Path::new("web-dist");
    if !dist.join("index.html").exists() {
        let _ = std::fs::create_dir_all(dist);
        let _ = std::fs::write(
            dist.join("index.html"),
            "<!doctype html><meta charset=utf-8><title>Dot Home</title>\
             <p>The portal wasn't built into this binary. Run <code>npm ci && npm run build</code> \
             in <code>home-web/</code>, then rebuild <code>dot-home</code>.</p>",
        );
    }
    // Release builds get their identity from CI; local builds report the crate version.
    for var in [
        "DOT_BUILD_VERSION",
        "DOT_BUILD_COMMIT",
        "DOT_BUILD_CHANNEL",
        "DOT_UPDATE_REPOSITORY",
        "DOT_UPDATE_PUBLIC_KEYS",
    ] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    println!("cargo:rerun-if-changed=web-dist");
    println!("cargo:rerun-if-changed=packs");
}
