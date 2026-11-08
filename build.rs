fn main() {
    // Exe icon (resource id 1, also used for the window) and version info.
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    res.set("FileDescription", "Whisker – mouse control");
    res.set("ProductName", "Whisker");
    if let Err(e) = res.compile() {
        println!("cargo:warning=icon resource skipped: {e}");
    }
}
