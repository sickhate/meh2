// GPL-3.0-or-later
fn main() {
    // The shader widget calls epoxy_get_proc_addr to load GL function pointers.
    // GTK4 always ships libepoxy, so this is always available on supported systems.
    if std::env::var("CARGO_FEATURE_SHADER").is_ok() {
        // eglGetProcAddress used for GL function loading in the shader widget.
        println!("cargo:rustc-link-lib=EGL");
    }
}
