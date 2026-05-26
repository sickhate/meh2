// GPL-3.0-or-later
//! CSS loading and application.

pub fn load_css(provider: &gtk4::CssProvider, css: &str) {
    provider.load_from_string(css);
}

pub fn apply_provider_to_display(provider: &gtk4::CssProvider) {
    if let Some(display) = gtk4::gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            provider,
            gtk4::STYLE_PROVIDER_PRIORITY_USER,
        );
    }
}
