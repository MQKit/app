//! The program's file details (Properties, Details in Explorer): who made it and which
//! version. Windows, SmartScreen and antivirus programs read them.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = tauri_winres::WindowsResource::new();
    res.set("CompanyName", "Software Hive S.R.L.")
        .set("ProductName", "MQKit")
        .set("FileDescription", "MQKit tools")
        .set("LegalCopyright", "Copyright (C) 2026 Software Hive S.R.L.")
        .set("OriginalFilename", "worker-core.exe");
    res.compile().expect("the version resource");
}
