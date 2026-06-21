fn main() {
    println!("cargo:rerun-if-changed=src/ui/main.slint");
    println!("cargo:rerun-if-changed=src/ui/settings.slint");
    println!("cargo:rerun-if-changed=packager/icon.ico");

    #[cfg(debug_assertions)]
    {
        unsafe { std::env::set_var("SLINT_LIVE_PREVIEW", "1") };
    }

    slint_build::compile("src/ui/main.slint")
        .expect("Failed to compile Slint UI");

    // Windows 平台将图标嵌入 exe 资源，使任务栏、窗口标题与桌面快捷方式显示同一图标
    #[cfg(target_os = "windows")]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("packager/icon.ico");
        res.set("FileDescription", "CCopy");
        res.set("ProductName", "CCopy");
        if let Err(e) = res.compile() {
            eprintln!("cargo:warning=failed to embed windows resource: {e}");
        }
    }
}
