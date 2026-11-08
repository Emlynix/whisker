fn main() {
    // Resources exist only in Windows executables.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    #[cfg(windows)]
    windows_resource();
}

/// Exe icon (resource id 1, also used for the window) and version info.
#[cfg(windows)]
fn windows_resource() {
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    res.set("FileDescription", "Whisker – mouse control");
    res.set("ProductName", "Whisker");
    if let Err(e) = res.compile() {
        println!("cargo:warning=icon resource skipped: {e}");
    }
}
