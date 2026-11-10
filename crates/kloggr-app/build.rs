//! On Windows, embeds the app icon and version information in kloggr.exe.

fn main() {
    println!("cargo:rerun-if-changed=../../packaging/windows/kloggr.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../packaging/windows/kloggr.ico")
            .set("ProductName", "kloggr")
            .set("FileDescription", "kloggr log viewer");
        res.compile()
            .expect("failed to embed the Windows resources");
    }
}
