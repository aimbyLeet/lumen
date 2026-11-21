//! The app icon into lumen-client.exe (lumen.rc): Explorer shows it, and the
//! video window takes it (its class icon, resource 1) for the taskbar.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("lumen.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("lumen.rc");
    }
}
