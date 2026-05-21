fn main() {
    slint_build::compile("ui/main_window.slint").unwrap();

    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("icons/icon.ico");
        res.set("FileDescription", "URL Album 3 — Bookmark Manager");
        res.set("ProductName", "URL Album 3");
        res.set("LegalCopyright", "URL Album 3");
        let _ = res.compile();
    }
}
