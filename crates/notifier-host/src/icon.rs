// MIT — ported from elkowar/eww crates/notifier_host/src/icon.rs
// GTK3-specific Pixbuf removed; returns IconResult for the GTK4 thread to consume.
use crate::{proxy, IconResult};

#[derive(thiserror::Error, Debug)]
enum IconError {
    #[error("fetching icon name: {0}")]
    DBusIconName(#[source] zbus::Error),
    #[error("fetching icon theme path: {0}")]
    DBusTheme(#[source] zbus::Error),
    #[error("fetching pixmap: {0}")]
    DBusPixmap(#[source] zbus::Error),
    #[error("no icon available")]
    NotAvailable,
}

/// Convert ARGB32 SNI pixmap bytes to RGBA32 and pick the best-sized variant.
fn icon_from_pixmaps(pixmaps: Vec<(i32, i32, Vec<u8>)>, size: i32) -> Option<IconResult> {
    pixmaps
        .into_iter()
        .max_by(|(w1, h1, _), (w2, h2, _)| {
            let a = size * size;
            let a1 = w1 * h1;
            let a2 = w2 * h2;
            match (a1 >= a, a2 >= a) {
                (true, true)   => a2.cmp(&a1),
                (true, false)  => std::cmp::Ordering::Greater,
                (false, true)  => std::cmp::Ordering::Less,
                (false, false) => a1.cmp(&a2),
            }
        })
        .map(|(w, h, mut data)| {
            // ARGB32 → RGBA32
            for chunk in data.chunks_exact_mut(4) {
                let a = chunk[0];
                let r = chunk[1];
                let g = chunk[2];
                let b = chunk[3];
                chunk[0] = r;
                chunk[1] = g;
                chunk[2] = b;
                chunk[3] = a;
            }
            IconResult::Pixmap { width: w, height: h, rgba: data }
        })
}

/// Load the icon for a StatusNotifierItem, returning an `IconResult` that the
/// GTK4 thread can turn into a widget.
///
/// Prefer icon-name (theme lookup) over pixmap, per the SNI specification.
pub async fn load_icon_from_sni(
    sni: &proxy::StatusNotifierItemProxy<'_>,
    size: i32,
) -> IconResult {
    let icon_from_name: Result<IconResult, IconError> = (async {
        let icon_name = sni.icon_name().await;
        tracing::debug!("dbus: {} icon_name -> {:?}", sni.destination(), icon_name);
        let icon_name = match icon_name {
            Ok(s) if s.is_empty() => return Err(IconError::NotAvailable),
            Ok(s) => s,
            Err(e) => return Err(IconError::DBusIconName(e)),
        };

        // Absolute path → the GTK thread will load the file directly.
        if std::path::Path::new(&icon_name).is_absolute() {
            return Ok(IconResult::Named { name: icon_name, theme_path: None });
        }

        let icon_theme_path = sni.icon_theme_path().await;
        tracing::debug!("dbus: {} icon_theme_path -> {:?}", sni.destination(), icon_theme_path);
        let theme_path = match icon_theme_path {
            Ok(p) if p.is_empty() => None,
            Ok(p) => Some(p),
            Err(zbus::Error::FDO(e)) => match *e {
                zbus::fdo::Error::UnknownProperty(_)
                | zbus::fdo::Error::InvalidArgs(_) => None,
                // discord, blueman-applet report this
                zbus::fdo::Error::Failed(ref msg) if msg == "error occurred in Get" => None,
                _ => return Err(IconError::DBusTheme(zbus::Error::FDO(e))),
            },
            Err(e) => return Err(IconError::DBusTheme(e)),
        };

        Ok(IconResult::Named { name: icon_name, theme_path })
    })
    .await;

    match icon_from_name {
        Ok(r) => return r,
        Err(IconError::NotAvailable) => {}
        Err(e) => tracing::warn!("icon by name failed for {}: {:?}", sni.destination(), e),
    }

    let icon_from_pixmap = match sni.icon_pixmap().await {
        Ok(ps) => match icon_from_pixmaps(ps, size) {
            Some(r) => Ok(r),
            None => Err(IconError::NotAvailable),
        },
        Err(zbus::Error::FDO(e)) => match *e {
            zbus::fdo::Error::UnknownProperty(_) | zbus::fdo::Error::InvalidArgs(_) => {
                Err(IconError::NotAvailable)
            }
            _ => Err(IconError::DBusPixmap(zbus::Error::FDO(e))),
        },
        Err(e) => Err(IconError::DBusPixmap(e)),
    };

    match icon_from_pixmap {
        Ok(r) => return r,
        Err(IconError::NotAvailable) => {}
        Err(e) => tracing::warn!("icon pixmap failed for {}: {:?}", sni.destination(), e),
    }

    IconResult::Missing
}
