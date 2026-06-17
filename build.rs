fn main() {
    #[cfg(debug_assertions)]
    {
        unsafe { std::env::set_var("SLINT_LIVE_PREVIEW", "1") };
    }

    slint_build::compile("src/ui/main.slint")
        .expect("Failed to compile Slint UI");
}
