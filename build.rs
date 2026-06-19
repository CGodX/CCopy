fn main() {
    println!("cargo:rerun-if-changed=src/ui/main.slint");
    println!("cargo:rerun-if-changed=src/ui/settings.slint");

    #[cfg(debug_assertions)]
    {
        unsafe { std::env::set_var("SLINT_LIVE_PREVIEW", "1") };
    }

    slint_build::compile("src/ui/main.slint")
        .expect("Failed to compile Slint UI");
}
