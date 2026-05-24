fn main() {
    slint_build::compile("ui/main_window.slint").unwrap();

    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("icons/icon.ico");
        res.set("FileDescription", "URL Album 3 — Bookmark Manager");
        res.set("ProductName", "URL Album 3");
        res.set("LegalCopyright", "URL Album 3");
        // DPI awareness: PerMonitorV2 (Win10 1607+), PerMonitor (Win8.1+), System (Win7+)
        res.set_manifest(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0" xmlns:asmv3="urn:schemas-microsoft-com:asm.v3">
  <asmv3:application>
    <asmv3:windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/PM</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2, PerMonitor, System</dpiAwareness>
    </asmv3:windowsSettings>
  </asmv3:application>
</assembly>"#);
        let _ = res.compile();

        // Win7 compatibility: our __imp_* overrides in compat.rs redirect calls
        // to local stubs.  /FORCE:MULTIPLE lets them win over the embedded import
        // lib stubs from libstd.rlib and windows_core.rlib.
        // /DELAYLOAD moves each DLL from the startup import table to the lazy
        // table, so Windows never demands them at process start — and because our
        // __imp_* pointers never trigger the delay-load thunk, the DLLs are never
        // actually loaded on Win7.
        println!("cargo:rustc-link-arg=/FORCE:MULTIPLE");
        println!("cargo:rustc-link-arg=/DELAYLOAD:api-ms-win-core-synch-l1-2-0.dll");
        println!("cargo:rustc-link-arg=/DELAYLOAD:bcryptprimitives.dll");
        println!("cargo:rustc-link-arg=/DELAYLOAD:api-ms-win-core-winrt-error-l1-1-0.dll");
        println!("cargo:rustc-link-arg=/DELAYLOAD:combase.dll");
        // CRT runtime DLL — delay-load so missing VS2019 UCRT functions
        // are never called through the DLL (our __imp_* stubs intercept them).
        println!("cargo:rustc-link-arg=/DELAYLOAD:api-ms-win-crt-runtime-l1-1-0.dll");
        println!("cargo:rustc-link-lib=delayimp");
        // __imp_* LTO protection is handled in compat::init() via black_box.
    }
}
