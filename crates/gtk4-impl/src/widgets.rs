// GPL-3.0-or-later
//! Individual GTK widget builders.

#[cfg(feature = "systray")]
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use anyhow::Result;
use gtk4::{gdk, prelude::*};
use meh_core::EvalCtx;
use yuck::config::widget_use::{BasicWidgetUse, WidgetUse};

use crate::bindings::maybe_bind;
use crate::builder::{apply_common_props, build_loop_oriented, build_widget};
use crate::runtime::spawn_cmd;
#[cfg(feature = "systray")]
use crate::runtime::TOKIO_HANDLE;

#[cfg(feature = "animations")]
use libadwaita::prelude::AnimationExt;

pub(crate) fn parse_align(s: &str) -> gtk4::Align {
    match s.to_lowercase().as_str() {
        "start" | "left" | "top" => gtk4::Align::Start,
        "end" | "right" | "bottom" => gtk4::Align::End,
        "center" => gtk4::Align::Center,
        "fill" | "baseline" => gtk4::Align::Fill,
        _ => gtk4::Align::Fill,
    }
}

pub(crate) fn parse_orientation(s: &str) -> gtk4::Orientation {
    match s.to_lowercase().as_str() {
        "h" | "horizontal" => gtk4::Orientation::Horizontal,
        _ => gtk4::Orientation::Vertical,
    }
}

// ── Box ───────────────────────────────────────────────────────────────────────

pub(crate) fn make_box_widget(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Box> {
    let attrs = &wu.attrs;
    let orientation = ctx
        .eval_attr_str(attrs, "orientation")
        .as_deref()
        .map(parse_orientation)
        .unwrap_or(gtk4::Orientation::Horizontal);
    let spacing = ctx.eval_attr_i64(attrs, "spacing").unwrap_or(0) as i32;

    let b = gtk4::Box::new(orientation, spacing);

    if let Some(v) = ctx.eval_attr_bool(attrs, "space-evenly") {
        b.set_homogeneous(v);
    }
    Ok(b)
}

pub(crate) fn build_box(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Box> {
    let b = make_box_widget(wu, ctx)?;
    let orientation = b.orientation();
    for child in &wu.children {
        let result = match child {
            WidgetUse::Loop(lp) => build_loop_oriented(lp, ctx, orientation),
            _ => build_widget(child, ctx),
        };
        match result {
            Ok(w) => b.append(&w),
            Err(e) => tracing::warn!("box child error: {}", e),
        }
    }
    apply_common_props(&b, wu, ctx);
    Ok(b)
}

// ── CenterBox ─────────────────────────────────────────────────────────────────

pub(crate) fn build_centerbox(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::CenterBox> {
    let b = gtk4::CenterBox::new();
    let attrs = &wu.attrs;
    let orientation = ctx
        .eval_attr_str(attrs, "orientation")
        .as_deref()
        .map(parse_orientation)
        .unwrap_or(gtk4::Orientation::Horizontal);
    b.set_orientation(orientation);

    let mut children = wu.children.iter().filter_map(|c| build_widget(c, ctx).ok());

    if let Some(w) = children.next() {
        b.set_start_widget(Some(&w));
    }
    if let Some(w) = children.next() {
        b.set_center_widget(Some(&w));
    }
    if let Some(w) = children.next() {
        b.set_end_widget(Some(&w));
    }

    apply_common_props(&b, wu, ctx);
    Ok(b)
}

// ── EventBox ─────────────────────────────────────────────────────────────────

pub(crate) fn build_eventbox(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Box> {
    let b = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    for child in &wu.children {
        match build_widget(child, ctx) {
            Ok(w) => b.append(&w),
            Err(e) => tracing::warn!("eventbox child error: {}", e),
        }
    }
    apply_common_props(&b, wu, ctx);
    Ok(b)
}

// ── Label ─────────────────────────────────────────────────────────────────────

#[allow(deprecated)]
pub(crate) fn build_label(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Label> {
    let attrs = &wu.attrs;
    let text = ctx.eval_attr_str(attrs, "text").unwrap_or_default();
    let label = gtk4::Label::new(None);

    let markup = ctx.eval_attr_bool(attrs, "markup").unwrap_or(false);
    let use_markup = ctx.eval_attr_bool(attrs, "use-markup").unwrap_or(markup);

    if use_markup {
        label.set_markup(&text);
    } else {
        label.set_text(&text);
    }

    // Reactive binding: re-evaluate text expression when vars change.
    {
        let lc = label.clone();
        maybe_bind(attrs, "text", &ctx.scope, text.clone(), move |v| {
            if use_markup {
                lc.set_markup(&v);
            } else {
                lc.set_text(&v);
            }
        });
    }

    if let Some(w) = ctx.eval_attr_i64(attrs, "limit-width") {
        label.set_max_width_chars(w as i32);
        label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    }
    if let Some(w) = ctx.eval_attr_i64(attrs, "width-chars") {
        label.set_width_chars(w as i32);
    }
    if let Some(justify) = ctx.eval_attr_str(attrs, "justify") {
        label.set_justify(match justify.as_str() {
            "left" => gtk4::Justification::Left,
            "right" => gtk4::Justification::Right,
            "center" => gtk4::Justification::Center,
            "fill" => gtk4::Justification::Fill,
            _ => gtk4::Justification::Left,
        });
    }
    if ctx.eval_attr_bool(attrs, "wrap").unwrap_or(false) {
        label.set_wrap(true);
        label.set_wrap_mode(gtk4::pango::WrapMode::Word);
    }
    // Note: gtk4::Label removed set_angle; use a rotation transform via CSS instead
    if let Some(deg) = ctx.eval_attr_f64(attrs, "angle") {
        let style = format!("transform: rotate({}deg);", deg);
        let provider = gtk4::CssProvider::new();
        provider.load_from_string(&format!("label {{ {} }}", style));
        label
            .style_context()
            .add_provider(&provider, gtk4::STYLE_PROVIDER_PRIORITY_USER);
    }

    apply_common_props(&label, wu, ctx);
    Ok(label)
}

// ── Button ────────────────────────────────────────────────────────────────────

pub(crate) fn build_button(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Button> {
    let button = gtk4::Button::new();
    // Build content from children or label attribute
    if wu.children.is_empty() {
        if let Some(text) = ctx.eval_attr_str(&wu.attrs, "label") {
            button.set_label(&text);
            let bc = button.clone();
            maybe_bind(&wu.attrs, "label", &ctx.scope, text, move |v| {
                bc.set_label(&v);
            });
        }
    } else {
        let container = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        for child in &wu.children {
            match build_widget(child, ctx) {
                Ok(w) => {
                    container.append(&w);
                }
                Err(e) => tracing::warn!("button child error: {}", e),
            }
        }
        button.set_child(Some(&container));
    }
    apply_common_props(&button, wu, ctx);
    Ok(button)
}

// ── Image ─────────────────────────────────────────────────────────────────────

pub(crate) fn build_image(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Widget> {
    let attrs = &wu.attrs;

    if let Some(path) = ctx.eval_attr_str(attrs, "path") {
        // Use DrawingArea + Cairo for file images. gtk4::Image and gtk4::Picture
        // both mis-size on fractional-scale displays (1.5x etc.) because the texture's
        // reported logical size is physical/scale, not the requested logical dimensions.
        // DrawingArea's draw-func receives the correctly-allocated logical (w, h) and
        // Cairo scales correctly to device pixels — no HiDPI arithmetic needed here.
        let area = gtk4::DrawingArea::new();

        // Shared mutable path + cached pixbuf for the draw func.
        let shared: std::rc::Rc<std::cell::RefCell<(String, Option<gdk::gdk_pixbuf::Pixbuf>)>> =
            std::rc::Rc::new(std::cell::RefCell::new((path.clone(), None)));

        let shared_draw = shared.clone();
        area.set_draw_func(move |_, cr, w, h| {
            if w <= 0 || h <= 0 {
                return;
            }
            let mut state = shared_draw.borrow_mut();
            // Load (or reload) pixbuf at the actual logical dimensions.
            let needs_load = state
                .1
                .as_ref()
                .map(|pb| pb.width() != w || pb.height() != h)
                .unwrap_or(true);
            if needs_load {
                #[allow(deprecated)]
                let pb = gdk::gdk_pixbuf::Pixbuf::from_file_at_scale(&state.0, w, h, false).ok();
                state.1 = pb;
            }
            if let Some(pb) = &state.1 {
                cr.set_source_pixbuf(pb, 0.0, 0.0);
                let _ = cr.paint();
            }
        });

        // Reactive path — invalidate cache and queue redraw.
        let shared_bind = shared.clone();
        let area_bind = area.clone();
        maybe_bind(attrs, "path", &ctx.scope, path, move |new_path| {
            shared_bind.borrow_mut().0 = new_path;
            shared_bind.borrow_mut().1 = None; // invalidate
            area_bind.queue_draw();
        });

        apply_common_props(&area, wu, ctx);
        return Ok(area.upcast());
    }

    // Icon-name path — keep gtk4::Image for themed icon rendering.
    #[allow(deprecated)]
    let image = gtk4::Image::new();
    if let Some(icon) = ctx.eval_attr_str(attrs, "icon-name") {
        let size = ctx
            .eval_attr_i64(attrs, "icon-size")
            .map(|s| s as i32)
            .unwrap_or(16);
        image.set_from_gicon(&gtk4::gio::ThemedIcon::new(&icon).upcast::<gtk4::gio::Icon>());
        image.set_pixel_size(size);
    }
    apply_common_props(&image, wu, ctx);
    Ok(image.upcast())
}

// ── Scale ─────────────────────────────────────────────────────────────────────

pub(crate) fn build_scale(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Scale> {
    let attrs = &wu.attrs;
    let orientation = ctx
        .eval_attr_str(attrs, "orientation")
        .as_deref()
        .map(parse_orientation)
        .unwrap_or(gtk4::Orientation::Horizontal);
    let min = ctx.eval_attr_f64(attrs, "min").unwrap_or(0.0);
    let max = ctx.eval_attr_f64(attrs, "max").unwrap_or(100.0);
    let value = ctx.eval_attr_f64(attrs, "value").unwrap_or(0.0);

    let adj = gtk4::Adjustment::new(value, min, max, 1.0, 10.0, 0.0);
    let scale = gtk4::Scale::new(orientation, Some(&adj));
    scale.set_draw_value(false);

    if let Some(inverted) = ctx.eval_attr_bool(attrs, "flipped") {
        scale.set_inverted(inverted);
    }
    if let Some(inverted) = ctx.eval_attr_bool(attrs, "inverted") {
        scale.set_inverted(inverted);
    }

    if let Some(cmd) = ctx.eval_attr_str(attrs, "onchange") {
        scale.connect_value_changed(move |s| {
            spawn_cmd(&cmd.replace("{}", &s.value().to_string()));
        });
    }

    {
        let sc = scale.clone();
        maybe_bind(attrs, "value", &ctx.scope, value.to_string(), move |v| {
            if let Ok(f) = v.parse::<f64>() {
                sc.set_value(f);
            }
        });
    }

    apply_common_props(&scale, wu, ctx);
    Ok(scale)
}

// ── Animations helpers (feature-gated) ────────────────────────────────────────

#[cfg(feature = "animations")]
pub(crate) fn ensure_adw_init() {
    static ADW_INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ADW_INIT.get_or_init(|| {
        libadwaita::init().expect("libadwaita init failed");
    });
}

#[cfg(feature = "animations")]
pub(crate) fn parse_adw_easing(s: Option<&str>) -> libadwaita::Easing {
    match s
        .unwrap_or("ease-in-out-cubic")
        .to_lowercase()
        .replace('_', "-")
        .as_str()
    {
        "linear" => libadwaita::Easing::Linear,
        "ease-in" | "ease-in-quad" => libadwaita::Easing::EaseInQuad,
        "ease-out" | "ease-out-quad" => libadwaita::Easing::EaseOutQuad,
        "ease-in-out" | "ease-in-out-quad" => libadwaita::Easing::EaseInOutQuad,
        "ease-in-cubic" => libadwaita::Easing::EaseInCubic,
        "ease-out-cubic" => libadwaita::Easing::EaseOutCubic,
        "ease-in-out-cubic" => libadwaita::Easing::EaseInOutCubic,
        "ease-in-sine" => libadwaita::Easing::EaseInSine,
        "ease-out-sine" => libadwaita::Easing::EaseOutSine,
        "ease-in-out-sine" => libadwaita::Easing::EaseInOutSine,
        "ease-in-expo" => libadwaita::Easing::EaseInExpo,
        "ease-out-expo" => libadwaita::Easing::EaseOutExpo,
        "ease-in-out-expo" => libadwaita::Easing::EaseInOutExpo,
        "ease-in-back" => libadwaita::Easing::EaseInBack,
        "ease-out-back" => libadwaita::Easing::EaseOutBack,
        "ease-in-out-back" => libadwaita::Easing::EaseInOutBack,
        "ease-in-bounce" => libadwaita::Easing::EaseInBounce,
        "ease-out-bounce" => libadwaita::Easing::EaseOutBounce,
        "ease-in-out-bounce" => libadwaita::Easing::EaseInOutBounce,
        _ => libadwaita::Easing::EaseInOutCubic,
    }
}

// ── Progress ──────────────────────────────────────────────────────────────────

pub(crate) fn build_progress(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::ProgressBar> {
    let attrs = &wu.attrs;
    let bar = gtk4::ProgressBar::new();

    let value = ctx.eval_attr_f64(attrs, "value").unwrap_or(0.0);
    bar.set_fraction((value / 100.0).clamp(0.0, 1.0));

    #[cfg(feature = "animations")]
    {
        let duration = ctx.eval_attr_i64(attrs, "animate-duration").unwrap_or(0) as u32;
        let easing = parse_adw_easing(ctx.eval_attr_str(attrs, "animate-easing").as_deref());
        if duration > 0 {
            ensure_adw_init();
            let anim_holder: std::rc::Rc<std::cell::RefCell<Option<libadwaita::TimedAnimation>>> =
                std::rc::Rc::new(std::cell::RefCell::new(None));
            let ah = anim_holder.clone();
            let bc = bar.clone();
            maybe_bind(attrs, "value", &ctx.scope, value.to_string(), move |v| {
                if let Ok(f) = v.parse::<f64>() {
                    let to = (f / 100.0).clamp(0.0, 1.0);
                    if let Some(p) = ah.borrow().as_ref() {
                        p.pause();
                    }
                    let from = bc.fraction();
                    let bc2 = bc.clone();
                    let target = libadwaita::CallbackAnimationTarget::new(move |val| {
                        bc2.set_fraction(val);
                    });
                    let a = libadwaita::TimedAnimation::new(&bc, from, to, duration, target);
                    a.set_easing(easing);
                    a.set_follow_enable_animations_setting(true);
                    a.play();
                    *ah.borrow_mut() = Some(a);
                }
            });
        } else {
            let bc = bar.clone();
            maybe_bind(attrs, "value", &ctx.scope, value.to_string(), move |v| {
                if let Ok(f) = v.parse::<f64>() {
                    bc.set_fraction((f / 100.0).clamp(0.0, 1.0));
                }
            });
        }
    }
    #[cfg(not(feature = "animations"))]
    {
        let bc = bar.clone();
        maybe_bind(attrs, "value", &ctx.scope, value.to_string(), move |v| {
            if let Ok(f) = v.parse::<f64>() {
                bc.set_fraction((f / 100.0).clamp(0.0, 1.0));
            }
        });
    }

    let orientation = ctx
        .eval_attr_str(attrs, "orientation")
        .as_deref()
        .map(parse_orientation)
        .unwrap_or(gtk4::Orientation::Horizontal);
    bar.set_orientation(orientation);

    if let Some(inverted) = ctx.eval_attr_bool(attrs, "flipped") {
        bar.set_inverted(inverted);
    }

    apply_common_props(&bar, wu, ctx);
    Ok(bar)
}

// ── Circular Progress (custom widget from Ewwii) ──────────────────────────────

static RING_OVERLAY_CTR: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub(crate) fn build_circular_progress(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Widget> {
    use std::{cell::Cell, f64::consts::PI, rc::Rc};
    let attrs = &wu.attrs;

    let value = ctx.eval_attr_f64(attrs, "value").unwrap_or(0.0);
    let thickness = ctx.eval_attr_f64(attrs, "thickness").unwrap_or(4.0);
    let clockwise = ctx.eval_attr_bool(attrs, "clockwise").unwrap_or(true);
    // `:start-at` (eww %) and `:start-angle` (degrees) both supported.
    // eww's start-at: 0=top, 25=right, 50=bottom, 75=left (percentage of circle).
    let start_angle = if let Some(pct) = ctx.eval_attr_f64(attrs, "start-at") {
        pct / 100.0 * 360.0 - 90.0
    } else {
        ctx.eval_attr_f64(attrs, "start-angle").unwrap_or(-90.0)
    };
    let explicit_fg = ctx.eval_attr_str(attrs, "foreground");
    let explicit_bg = ctx.eval_attr_str(attrs, "background");

    let size = ctx
        .eval_attr_i64(attrs, "width")
        .or_else(|| ctx.eval_attr_i64(attrs, "height"))
        .unwrap_or(24) as i32;

    let area = gtk4::DrawingArea::new();
    area.set_content_width(size);
    area.set_content_height(size);
    // No CSS provider needed: draw_func clears to transparent (Source operator)
    // before painting, so the GTK theme background is always overwritten.

    // Shared reactive value — updated by maybe_bind, read by draw_func.
    let value_cell = Rc::new(Cell::new(value));
    {
        let vc = value_cell.clone();
        let area_ref = area.clone();
        maybe_bind(attrs, "value", &ctx.scope, value.to_string(), move |v| {
            if let Ok(f) = v.parse::<f64>() {
                vc.set(f);
                area_ref.queue_draw();
            }
        });
    }

    area.set_draw_func(move |widget, cr, w, h| {
        // Erase whatever GTK painted as CSS background — we control all drawing.
        cr.set_operator(gtk4::cairo::Operator::Source);
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
        cr.paint().ok();
        cr.set_operator(gtk4::cairo::Operator::Over);

        let val = value_cell.get();
        let w = w as f64;
        let h = h as f64;
        let cx = w / 2.0;
        let cy = h / 2.0;
        let r = (w.min(h) / 2.0) - thickness / 2.0;
        let start = start_angle * PI / 180.0;
        let fraction = (val / 100.0).clamp(0.0, 1.0);
        let sweep = 2.0 * PI * fraction;

        // Background arc (full circle) — use explicit attr or transparent
        let bg_rgba = if let Some(ref c) = explicit_bg {
            parse_rgba_css(c)
        } else {
            (0.0, 0.0, 0.0, 0.0)
        };
        if bg_rgba.3 > 0.0 {
            cr.set_source_rgba(bg_rgba.0, bg_rgba.1, bg_rgba.2, bg_rgba.3);
            cr.set_line_width(thickness);
            cr.arc(cx, cy, r, 0.0, 2.0 * PI);
            let _ = cr.stroke();
        }

        // Foreground arc — use explicit attr, else read CSS `color` property
        let fg_rgba = if let Some(ref c) = explicit_fg {
            parse_rgba_css(c)
        } else {
            let c = widget.color();
            (
                c.red() as f64,
                c.green() as f64,
                c.blue() as f64,
                c.alpha() as f64,
            )
        };
        // Only draw if there's something to draw — cairo arc with equal angles draws a full circle.
        if sweep > 1e-6 {
            cr.set_source_rgba(fg_rgba.0, fg_rgba.1, fg_rgba.2, fg_rgba.3);
            cr.set_line_width(thickness);
            if clockwise {
                cr.arc(cx, cy, r, start, start + sweep);
            } else {
                cr.arc_negative(cx, cy, r, start, start - sweep);
            }
            let _ = cr.stroke();
        }
    });

    // If children present, wrap in an Overlay so icons/labels render on top.
    // CSS class goes on the Overlay when children are present:
    //   - `color` IS inherited → DrawingArea.widget.color() picks up the user's
    //     color rule (used for the foreground arc in draw_func).
    //   - `background-color` is NOT inherited → the user's `.stat-ring { background-color }
    //     rule on the Overlay never paints a grey rectangle behind the Cairo arcs.
    let widget: gtk4::Widget = if wu.children.is_empty() {
        apply_common_props(&area, wu, ctx);
        area.upcast()
    } else {
        let overlay = gtk4::Overlay::new();
        overlay.set_child(Some(&area));
        for child in &wu.children {
            if let Ok(w) = build_widget(child, ctx) {
                w.set_halign(gtk4::Align::Center);
                w.set_valign(gtk4::Align::Center);
                overlay.add_overlay(&w);
            }
        }
        overlay.set_size_request(size, size);
        // Force overlay background transparent at priority 901 (> user stylesheet 800).
        // GTK4 4.10+: style_context().add_provider() is deprecated; use a unique class
        // scoped to this widget instance and a display-level provider instead.
        let n = RING_OVERLAY_CTR.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let ov_class = format!("meh-ring-ov-{n}");
        overlay.add_css_class(&ov_class);
        let ov_provider = gtk4::CssProvider::new();
        ov_provider.load_from_string(&format!(".{ov_class} {{ background: transparent; }}"));
        if let Some(display) = gdk::Display::default() {
            gtk4::style_context_add_provider_for_display(&display, &ov_provider, 901);
            overlay.connect_destroy(move |_| {
                if let Some(d) = gdk::Display::default() {
                    gtk4::style_context_remove_provider_for_display(&d, &ov_provider);
                }
            });
        }
        // CSS class (e.g. .stat-ring) and onclick/events go on the overlay wrapper.
        apply_common_props(&overlay, wu, ctx);
        overlay.upcast()
    };

    Ok(widget)
}

pub(crate) fn parse_rgba_css(s: &str) -> (f64, f64, f64, f64) {
    let s = s.trim();
    // Try GTK's built-in parser first — handles #hex, rgb(), rgba(), named colors.
    if let Ok(rgba) = s.parse::<gtk4::gdk::RGBA>() {
        return (
            rgba.red() as f64,
            rgba.green() as f64,
            rgba.blue() as f64,
            rgba.alpha() as f64,
        );
    }
    // Manual hex fallback
    if s.starts_with('#') {
        let hex = s.trim_start_matches('#');
        let (r, g, b, a) = match hex.len() {
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(255) as f64 / 255.0;
                let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(255) as f64 / 255.0;
                let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(255) as f64 / 255.0;
                (r, g, b, 1.0)
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(255) as f64 / 255.0;
                let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(255) as f64 / 255.0;
                let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(255) as f64 / 255.0;
                let a = u8::from_str_radix(&hex[6..8], 16).unwrap_or(255) as f64 / 255.0;
                (r, g, b, a)
            }
            _ => (1.0, 1.0, 1.0, 1.0),
        };
        return (r, g, b, a);
    }
    // transparent fallback
    (0.0, 0.0, 0.0, 0.0)
}

// ── Scroll ────────────────────────────────────────────────────────────────────

pub(crate) fn build_scroll(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::ScrolledWindow> {
    let sw = gtk4::ScrolledWindow::new();
    let attrs = &wu.attrs;

    let hscroll = ctx.eval_attr_bool(attrs, "hscroll").unwrap_or(true);
    let vscroll = ctx.eval_attr_bool(attrs, "vscroll").unwrap_or(true);
    sw.set_policy(
        if hscroll {
            gtk4::PolicyType::Automatic
        } else {
            gtk4::PolicyType::Never
        },
        if vscroll {
            gtk4::PolicyType::Automatic
        } else {
            gtk4::PolicyType::Never
        },
    );

    if let Some(child) = wu.children.first()
        && let Ok(w) = build_widget(child, ctx)
    {
        sw.set_child(Some(&w));
    }

    apply_common_props(&sw, wu, ctx);
    Ok(sw)
}

// ── Overlay ───────────────────────────────────────────────────────────────────

pub(crate) fn build_overlay(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Overlay> {
    let overlay = gtk4::Overlay::new();
    let mut iter = wu.children.iter();
    if let Some(main) = iter.next()
        && let Ok(w) = build_widget(main, ctx)
    {
        overlay.set_child(Some(&w));
    }
    for extra in iter {
        if let Ok(w) = build_widget(extra, ctx) {
            overlay.add_overlay(&w);
        }
    }
    apply_common_props(&overlay, wu, ctx);
    Ok(overlay)
}

// ── Revealer ──────────────────────────────────────────────────────────────────

pub(crate) fn build_revealer(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Revealer> {
    let revealer = gtk4::Revealer::new();
    let attrs = &wu.attrs;

    let transition = ctx
        .eval_attr_str(attrs, "transition")
        .as_deref()
        .map(parse_revealer_transition)
        .unwrap_or(gtk4::RevealerTransitionType::SlideDown);
    revealer.set_transition_type(transition);

    if let Some(dur) = ctx.eval_attr_i64(attrs, "duration") {
        revealer.set_transition_duration(dur as u32);
    }

    let reveal = ctx.eval_attr_bool(attrs, "reveal").unwrap_or(true);
    revealer.set_reveal_child(reveal);

    {
        let rc = revealer.clone();
        maybe_bind(attrs, "reveal", &ctx.scope, reveal.to_string(), move |v| {
            rc.set_reveal_child(v == "true");
        });
    }

    if let Some(child) = wu.children.first()
        && let Ok(w) = build_widget(child, ctx)
    {
        revealer.set_child(Some(&w));
    }

    apply_common_props(&revealer, wu, ctx);
    Ok(revealer)
}

pub(crate) fn parse_revealer_transition(s: &str) -> gtk4::RevealerTransitionType {
    match s.to_lowercase().as_str() {
        "slideright" | "slide_right" => gtk4::RevealerTransitionType::SlideRight,
        "slideleft" | "slide_left" => gtk4::RevealerTransitionType::SlideLeft,
        "slideup" | "slide_up" => gtk4::RevealerTransitionType::SlideUp,
        "crossfade" | "cross_fade" => gtk4::RevealerTransitionType::Crossfade,
        "none" => gtk4::RevealerTransitionType::None,
        _ => gtk4::RevealerTransitionType::SlideDown,
    }
}

// ── Stack ─────────────────────────────────────────────────────────────────────

pub(crate) fn build_stack(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Stack> {
    let stack = gtk4::Stack::new();
    let attrs = &wu.attrs;

    if let Some(transition) = ctx.eval_attr_str(attrs, "transition") {
        let t = match transition.as_str() {
            "slide-right" | "slideright" => gtk4::StackTransitionType::SlideRight,
            "slide-left" | "slideleft" => gtk4::StackTransitionType::SlideLeft,
            "slide-up" | "slideup" => gtk4::StackTransitionType::SlideUp,
            "slide-down" | "slidedown" => gtk4::StackTransitionType::SlideDown,
            "crossfade" => gtk4::StackTransitionType::Crossfade,
            _ => gtk4::StackTransitionType::None,
        };
        stack.set_transition_type(t);
    }

    if let Some(shown) = ctx.eval_attr_str(attrs, "shown") {
        for child in &wu.children {
            if let WidgetUse::Basic(b) = child
                && let Some(name) = ctx
                    .eval_attr_str(&b.attrs, "name")
                    .or_else(|| Some(b.name.clone()))
                && let Ok(w) = build_widget(child, ctx)
            {
                stack.add_named(&w, Some(&name));
            }
        }
        stack.set_visible_child_name(&shown);
    } else {
        for child in &wu.children {
            if let Ok(w) = build_widget(child, ctx) {
                stack.add_child(&w);
            }
        }
    }

    apply_common_props(&stack, wu, ctx);
    Ok(stack)
}

// ── Expander ──────────────────────────────────────────────────────────────────

pub(crate) fn build_expander(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Expander> {
    let attrs = &wu.attrs;
    let label = ctx.eval_attr_str(attrs, "label").unwrap_or_default();
    let exp = gtk4::Expander::new(Some(&label));
    exp.set_expanded(ctx.eval_attr_bool(attrs, "expanded").unwrap_or(false));

    if let Some(child) = wu.children.first()
        && let Ok(w) = build_widget(child, ctx)
    {
        exp.set_child(Some(&w));
    }

    apply_common_props(&exp, wu, ctx);
    Ok(exp)
}

// ── Checkbox ─────────────────────────────────────────────────────────────────

pub(crate) fn build_checkbox(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::CheckButton> {
    let attrs = &wu.attrs;
    let cb = gtk4::CheckButton::new();
    if let Some(v) = ctx.eval_attr_bool(attrs, "checked") {
        cb.set_active(v);
    }
    if let Some(label) = ctx.eval_attr_str(attrs, "label") {
        cb.set_label(Some(&label));
    }

    if let Some(cmd) = ctx.eval_attr_str(attrs, "ontoggle") {
        cb.connect_toggled(move |b| {
            spawn_cmd(&cmd.replace("{}", &b.is_active().to_string()));
        });
    }

    apply_common_props(&cb, wu, ctx);
    Ok(cb)
}

// ── Input ─────────────────────────────────────────────────────────────────────

pub(crate) fn build_input(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Entry> {
    let attrs = &wu.attrs;
    let entry = gtk4::Entry::new();

    // :value — reactive; bind a var to clear/reset the field (e.g. launcher pattern)
    let initial_value = ctx.eval_attr_str(attrs, "value").unwrap_or_default();
    entry.set_text(&initial_value);
    {
        let e = entry.clone();
        maybe_bind(attrs, "value", &ctx.scope, initial_value, move |v| {
            e.set_text(&v);
        });
    }

    if let Some(v) = ctx.eval_attr_str(attrs, "placeholder") {
        entry.set_placeholder_text(Some(&v));
    }

    if let Some(cmd) = ctx.eval_attr_str(attrs, "onchange") {
        entry.connect_changed(move |e| {
            spawn_cmd(&cmd.replace("{}", &e.text()));
        });
    }
    if let Some(cmd) = ctx.eval_attr_str(attrs, "onaccept") {
        entry.connect_activate(move |e| {
            spawn_cmd(&cmd.replace("{}", &e.text()));
        });
    }

    // :focus — grab keyboard focus on map (static "true") or when var flips to "true"
    // Useful for launchers: open the window, then `meh update FOCUS=true`
    let initial_focus = ctx.eval_attr_str(attrs, "focus").unwrap_or_default();
    if initial_focus == "true" {
        let e = entry.clone();
        entry.connect_map(move |_| {
            e.grab_focus();
        });
    }
    {
        let e = entry.clone();
        maybe_bind(attrs, "focus", &ctx.scope, initial_focus, move |v| {
            if v == "true" {
                e.grab_focus();
            }
        });
    }

    apply_common_props(&entry, wu, ctx);
    Ok(entry)
}

// ── Calendar ──────────────────────────────────────────────────────────────────

pub(crate) fn build_calendar(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Calendar> {
    let cal = gtk4::Calendar::new();
    if let Some(cmd) = ctx.eval_attr_str(&wu.attrs, "onclick") {
        cal.connect_day_selected(move |c| {
            let date = c.date();
            spawn_cmd(&cmd.replace(
                "{}",
                &format!(
                    "{}-{:02}-{:02}",
                    date.year(),
                    date.month() as u32,
                    date.day_of_month()
                ),
            ));
        });
    }
    apply_common_props(&cal, wu, ctx);
    Ok(cal)
}

// ── ComboBoxText ──────────────────────────────────────────────────────────────

#[allow(deprecated)]
pub(crate) fn build_combo_box_text(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::ComboBoxText> {
    let attrs = &wu.attrs;
    let combo = gtk4::ComboBoxText::new();

    if let Some(items) = ctx.eval_attr(attrs, "items")
        && let Ok(arr) = items.as_vec()
    {
        for item in &arr {
            combo.append_text(item);
        }
    }
    if let Some(active) = ctx.eval_attr_i64(attrs, "active") {
        combo.set_active(Some(active as u32));
    }

    if let Some(cmd) = ctx.eval_attr_str(attrs, "onchange") {
        combo.connect_changed(move |c| {
            let v = c.active_text().map(|s| s.to_string()).unwrap_or_default();
            spawn_cmd(&cmd.replace("{}", &v));
        });
    }

    apply_common_props(&combo, wu, ctx);
    Ok(combo)
}

// ── ColorButton ───────────────────────────────────────────────────────────────

#[allow(deprecated)]
pub(crate) fn build_color_button(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::ColorButton> {
    let cb = gtk4::ColorButton::new();
    if let Some(cmd) = ctx.eval_attr_str(&wu.attrs, "onchange") {
        cb.connect_color_set(move |b| {
            let c = b.rgba();
            spawn_cmd(&cmd.replace(
                "{}",
                &format!(
                    "rgba({},{},{},{})",
                    (c.red() * 255.0) as u8,
                    (c.green() * 255.0) as u8,
                    (c.blue() * 255.0) as u8,
                    c.alpha()
                ),
            ));
        });
    }
    apply_common_props(&cb, wu, ctx);
    Ok(cb)
}

// ── Literal ───────────────────────────────────────────────────────────────────

pub(crate) fn build_literal(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Label> {
    // `literal` is just a raw label that can contain markup
    let text = ctx.eval_attr_str(&wu.attrs, "content").unwrap_or_default();
    let label = gtk4::Label::new(None);
    label.set_markup(&text);
    {
        let lc = label.clone();
        maybe_bind(&wu.attrs, "content", &ctx.scope, text, move |v| {
            lc.set_markup(&v);
        });
    }
    apply_common_props(&label, wu, ctx);
    Ok(label)
}

// ── Systray ───────────────────────────────────────────────────────────────────

#[cfg(feature = "systray")]
pub(crate) fn build_systray(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Box> {
    use std::{
        collections::HashMap,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
    };

    let container = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    apply_common_props(&container.clone().upcast::<gtk4::Widget>(), wu, ctx);

    let (tx, rx) = std::sync::mpsc::channel::<SystrayEvent>();
    let rx = Arc::new(Mutex::new(rx));
    let pending = Arc::new(AtomicBool::new(false));

    let items: Arc<Mutex<HashMap<String, gtk4::Button>>> = Default::default();

    {
        let rx = rx.clone();
        let container = container.clone();
        let items = items.clone();
        let pending = pending.clone();
        gtk4::glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
            if !pending.swap(false, Ordering::AcqRel) {
                return gtk4::glib::ControlFlow::Continue;
            }
            while let Ok(ev) = rx.lock().unwrap().try_recv() {
                systray_handle_event(ev, &container, &items);
            }
            gtk4::glib::ControlFlow::Continue
        });
    }

    let tray_tx = TrayEventSender { tx, pending };

    let Some(handle) = TOKIO_HANDLE.get() else {
        tracing::warn!("systray: no tokio handle set; tray will be empty");
        return Ok(container);
    };

    handle.spawn(async move {
        if let Err(e) = run_notifier_host_task(tray_tx).await {
            tracing::error!("systray host error: {}", e);
        }
    });

    Ok(container)
}

#[cfg(feature = "systray")]
struct TrayEventSender {
    tx: std::sync::mpsc::Sender<SystrayEvent>,
    pending: Arc<AtomicBool>,
}

#[cfg(feature = "systray")]
impl Clone for TrayEventSender {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            pending: self.pending.clone(),
        }
    }
}

#[cfg(feature = "systray")]
impl TrayEventSender {
    fn send(&self, ev: SystrayEvent) {
        if self.tx.send(ev).is_ok() {
            self.pending.store(true, Ordering::Release);
        }
    }
}

#[cfg(feature = "systray")]
enum SystrayEvent {
    Add {
        id: String,
        icon: meh_notifier_host::IconResult,
        tooltip: Option<String>,
        item: meh_notifier_host::Item,
    },
    Remove {
        id: String,
    },
}

#[cfg(feature = "systray")]
pub(crate) fn systray_handle_event(
    ev: SystrayEvent,
    container: &gtk4::Box,
    items: &Arc<Mutex<std::collections::HashMap<String, gtk4::Button>>>,
) {
    use std::sync::Arc;
    match ev {
        SystrayEvent::Add {
            id,
            icon,
            tooltip,
            item,
        } => {
            let img = systray_icon_to_image(icon);
            img.set_pixel_size(24);

            let btn = gtk4::Button::new();
            btn.set_child(Some(&img));
            if let Some(tip) = tooltip {
                btn.set_tooltip_text(Some(&tip));
            }

            let item = Arc::new(item);
            btn.connect_clicked(move |_| {
                let item = item.clone();
                if let Some(handle) = TOKIO_HANDLE.get() {
                    handle.spawn(async move {
                        let _ = item.sni.activate(0, 0).await;
                    });
                }
            });

            container.append(&btn);
            items.lock().unwrap().insert(id, btn);
        }
        SystrayEvent::Remove { id } => {
            if let Some(btn) = items.lock().unwrap().remove(&id) {
                container.remove(&btn);
            }
        }
    }
}

#[cfg(feature = "systray")]
pub(crate) fn systray_icon_to_image(icon: meh_notifier_host::IconResult) -> gtk4::Image {
    use meh_notifier_host::IconResult;
    match icon {
        IconResult::Named {
            name,
            theme_path: None,
        } => {
            if std::path::Path::new(&name).is_absolute() {
                let img = gtk4::Image::new();
                if let Ok(texture) = gtk4::gdk::Texture::from_filename(&name) {
                    img.set_paintable(Some(&texture));
                }
                img
            } else {
                gtk4::Image::from_icon_name(&name)
            }
        }
        IconResult::Named {
            name,
            theme_path: Some(tp),
        } => {
            let theme = gtk4::IconTheme::new();
            theme.add_search_path(&tp);
            let paintable = theme.lookup_icon(
                &name,
                &[],
                24,
                1,
                gtk4::TextDirection::None,
                gtk4::IconLookupFlags::empty(),
            );
            gtk4::Image::from_paintable(Some(&paintable))
        }
        IconResult::Pixmap {
            width,
            height,
            rgba,
        } => {
            let bytes = gtk4::glib::Bytes::from_owned(rgba);
            let texture = gtk4::gdk::MemoryTexture::new(
                width,
                height,
                gtk4::gdk::MemoryFormat::R8g8b8a8,
                &bytes,
                (width * 4) as usize,
            );
            gtk4::Image::from_paintable(Some(&texture))
        }
        IconResult::Missing => gtk4::Image::from_icon_name("image-missing"),
    }
}

#[cfg(feature = "systray")]
async fn run_notifier_host_task(tray_tx: TrayEventSender) -> anyhow::Result<()> {
    use meh_notifier_host::{Host, Item, Watcher, register_as_host, run_host};

    struct MehHost {
        tx: TrayEventSender,
    }

    impl Host for MehHost {
        fn add_item(&mut self, id: &str, item: Item) {
            let id = id.to_owned();
            let tx = self.tx.clone();
            tokio::spawn(async move {
                let icon = item.load_icon_result(24).await;
                let tooltip = item.sni.title().await.ok().filter(|s| !s.is_empty());
                tx.send(SystrayEvent::Add {
                    id,
                    icon,
                    tooltip,
                    item,
                });
            });
        }

        fn remove_item(&mut self, id: &str) {
            self.tx.send(SystrayEvent::Remove { id: id.to_owned() });
        }
    }

    let con = zbus::Connection::session().await?;
    Watcher::new().attach_to(&con).await?;
    let (_name, snw) = register_as_host(&con).await?;
    let mut host = MehHost { tx: tray_tx };
    let err = run_host(&mut host, &snw).await;
    Err(anyhow::anyhow!("notifier host: {}", err))
}

// ── Shader widget (GtkGLArea + glow) ─────────────────────────────────────────

#[cfg(feature = "shader")]
const VERT_SRC: &str = concat!(
    "#version 330 core\n",
    "const vec2 VERTS[3] = vec2[3](vec2(-1,-1), vec2(3,-1), vec2(-1,3));\n",
    "void main() { gl_Position = vec4(VERTS[gl_VertexID], 0.0, 1.0); }\n",
);

#[cfg(feature = "shader")]
struct ShaderGlState {
    gl: glow::Context,
    program: glow::NativeProgram,
    vao: glow::NativeVertexArray,
    loc_time: Option<glow::NativeUniformLocation>,
    loc_resolution: Option<glow::NativeUniformLocation>,
    start: std::time::Instant,
    w: f32,
    h: f32,
}

/// Load GL function pointers via EGL (the GTK4 GL backend on Wayland).
/// Must be called only when an EGL context is current (inside `connect_realize`
/// after `area.make_current()`).
#[cfg(feature = "shader")]
pub(crate) fn egl_loader(name: &std::ffi::CStr) -> *const std::ffi::c_void {
    unsafe extern "C" {
        fn eglGetProcAddress(procname: *const std::ffi::c_char) -> *const std::ffi::c_void;
    }
    unsafe { eglGetProcAddress(name.as_ptr()) }
}

#[cfg(feature = "shader")]
pub(crate) fn compile_program(gl: &glow::Context, vert: &str, frag: &str) -> Result<glow::NativeProgram> {
    use glow::HasContext;
    unsafe {
        let vs = gl
            .create_shader(glow::VERTEX_SHADER)
            .map_err(|e| anyhow::anyhow!("create vertex shader: {}", e))?;
        gl.shader_source(vs, vert);
        gl.compile_shader(vs);
        if !gl.get_shader_compile_status(vs) {
            let log = gl.get_shader_info_log(vs);
            gl.delete_shader(vs);
            anyhow::bail!("vertex shader: {}", log);
        }

        let fs = gl
            .create_shader(glow::FRAGMENT_SHADER)
            .map_err(|e| anyhow::anyhow!("create fragment shader: {}", e))?;
        gl.shader_source(fs, frag);
        gl.compile_shader(fs);
        if !gl.get_shader_compile_status(fs) {
            let log = gl.get_shader_info_log(fs);
            gl.delete_shader(fs);
            gl.delete_shader(vs);
            anyhow::bail!("fragment shader: {}", log);
        }

        let prog = gl
            .create_program()
            .map_err(|e| anyhow::anyhow!("create program: {}", e))?;
        gl.attach_shader(prog, vs);
        gl.attach_shader(prog, fs);
        gl.link_program(prog);
        gl.detach_shader(prog, vs);
        gl.delete_shader(vs);
        gl.detach_shader(prog, fs);
        gl.delete_shader(fs);

        if !gl.get_program_link_status(prog) {
            let log = gl.get_program_info_log(prog);
            gl.delete_program(prog);
            anyhow::bail!("program link: {}", log);
        }
        Ok(prog)
    }
}

/// `(shader :frag "path/to/shader.glsl" :width 300 :height 200)`
///
/// Renders a GLSL fragment shader via GtkGLArea (OpenGL 3.3 core).
/// Available uniforms in the fragment shader:
/// - `uniform float iTime`      — seconds since the widget was realized
/// - `uniform vec2  iResolution` — widget dimensions in pixels
///
/// Only available in the `full` build profile (`--features shader`).
#[cfg(feature = "shader")]
pub(crate) fn build_shader(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::GLArea> {
    use glow::HasContext;
    use gtk4::prelude::*;
    use std::{cell::RefCell, rc::Rc};

    let frag_path = ctx
        .eval_attr_str(&wu.attrs, "frag")
        .ok_or_else(|| anyhow::anyhow!("`shader` widget requires `:frag` attribute"))?;

    // Solid magenta fallback — visually obvious indicator of a missing shader file.
    let frag_src = std::fs::read_to_string(&frag_path).unwrap_or_else(|e| {
        tracing::warn!("shader: cannot read `{}`: {}", frag_path, e);
        "#version 330 core\nout vec4 fragColor;\nvoid main(){fragColor=vec4(1,0,1,1);}".to_string()
    });

    let area = gtk4::GLArea::new();
    area.set_required_version(3, 3);
    area.set_use_es(false);
    area.set_has_depth_buffer(false);
    area.set_has_stencil_buffer(false);
    area.set_auto_render(true);

    if let Some(w) = ctx.eval_attr_i64(&wu.attrs, "width") {
        area.set_width_request(w as i32);
    }
    if let Some(h) = ctx.eval_attr_i64(&wu.attrs, "height") {
        area.set_height_request(h as i32);
    }
    apply_common_props(&area.clone().upcast::<gtk4::Widget>(), wu, ctx);

    let state: Rc<RefCell<Option<ShaderGlState>>> = Rc::new(RefCell::new(None));

    // realize: compile shaders, create VAO
    {
        let state = state.clone();
        area.connect_realize(move |area| {
            area.make_current();
            if area.error().is_some() {
                return;
            }
            let gl = unsafe { glow::Context::from_loader_function_cstr(egl_loader) };
            match compile_program(&gl, VERT_SRC, &frag_src) {
                Ok(program) => {
                    let vao = unsafe { gl.create_vertex_array().expect("create vao") };
                    let loc_time = unsafe { gl.get_uniform_location(program, "iTime") };
                    let loc_resolution = unsafe { gl.get_uniform_location(program, "iResolution") };
                    *state.borrow_mut() = Some(ShaderGlState {
                        gl,
                        program,
                        vao,
                        loc_time,
                        loc_resolution,
                        start: std::time::Instant::now(),
                        w: 1.0,
                        h: 1.0,
                    });
                }
                Err(e) => tracing::warn!("shader compile failed for `{}`: {}", frag_path, e),
            }
        });
    }

    // unrealize: free GL objects before the context is destroyed
    {
        let state = state.clone();
        area.connect_unrealize(move |area| {
            area.make_current();
            if let Some(s) = state.borrow_mut().take() {
                unsafe {
                    s.gl.delete_vertex_array(s.vao);
                    s.gl.delete_program(s.program);
                }
            }
        });
    }

    // resize: update iResolution
    {
        let state = state.clone();
        area.connect_resize(move |_area, w, h| {
            if let Some(s) = state.borrow_mut().as_mut() {
                s.w = w as f32;
                s.h = h as f32;
            }
        });
    }

    // render: draw one fullscreen triangle per frame
    {
        let state = state.clone();
        area.connect_render(move |_area, _gl_ctx| {
            let borrowed = state.borrow();
            if let Some(s) = borrowed.as_ref() {
                unsafe {
                    s.gl.clear_color(0.0, 0.0, 0.0, 0.0);
                    s.gl.clear(glow::COLOR_BUFFER_BIT);
                    s.gl.use_program(Some(s.program));
                    if let Some(loc) = s.loc_time.as_ref() {
                        s.gl.uniform_1_f32(Some(loc), s.start.elapsed().as_secs_f32());
                    }
                    if let Some(loc) = s.loc_resolution.as_ref() {
                        s.gl.uniform_2_f32(Some(loc), s.w, s.h);
                    }
                    s.gl.bind_vertex_array(Some(s.vao));
                    s.gl.draw_arrays(glow::TRIANGLES, 0, 3);
                }
            }
            gtk4::glib::Propagation::Proceed
        });
    }

    Ok(area)
}
