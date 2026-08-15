fn main() {
    // 嵌入应用图标与版本信息(仅 Windows;资源编译失败不阻塞构建)。
    #[cfg(target_os = "windows")]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/poe-alarm.ico");
        resource.set("ProductName", "POE Alarm");
        resource.set("FileDescription", "POE Alarm — OCR crafting alarm for Path of Exile");
        resource.set("LegalCopyright", "\u{a9} SouNd");
        if let Err(error) = resource.compile() {
            println!("cargo:warning=windows resource compilation failed: {error}");
        }
    }
    println!("cargo:rerun-if-changed=assets/poe-alarm.ico");
}
