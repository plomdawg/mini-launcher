//! Embeds the app icon in `mini-launcher.exe` on Windows. A no-op elsewhere.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(windows)]
    windows::embed_icon();
}

#[cfg(windows)]
mod windows {
    const ICON: &str = "../assets/icon/mini-launcher.ico";

    pub fn embed_icon() {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
            return;
        }
        println!("cargo:rerun-if-changed={ICON}");
        winresource::WindowsResource::new()
            .set_icon(ICON)
            .compile()
            .expect("failed to embed the Windows icon resource");
    }
}
