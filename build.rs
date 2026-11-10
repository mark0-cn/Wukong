fn main() {
    // Both clients embed these exact resources; no network fetch at runtime.
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed=locales");
    for locale in ["en-US", "zh-CN"] {
        let mut files: Vec<_> = std::fs::read_dir(format!("locales/{locale}"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|e| e == "ftl"))
            .collect();
        files.sort();
        let source = files
            .into_iter()
            .map(|path| std::fs::read_to_string(path).unwrap() + "\n")
            .collect::<String>();
        std::fs::write(output.join(format!("{locale}.ftl")), source).unwrap();
    }
    #[cfg(feature = "desktop")]
    tauri_build::build();
}
