// MIT — ported from elkowar/eww crates/notifier_host/src/item.rs
// dbusmenu_gtk3 removed (GTK3-only). Context menus deferred to Phase 2.
use crate::{IconResult, icon, proxy};

use serde::Deserialize;
use zbus::fdo::IntrospectableProxy;

/// Values of `org.freedesktop.StatusNotifierItem.Status`.
#[derive(Debug, Clone, Copy)]
pub enum Status {
    Passive,
    Active,
    NeedsAttention,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ParseStatusError;

impl std::str::FromStr for Status {
    type Err = ParseStatusError;
    fn from_str(s: &str) -> Result<Self, ParseStatusError> {
        match s {
            "Passive" => Ok(Status::Passive),
            "Active" => Ok(Status::Active),
            "NeedsAttention" => Ok(Status::NeedsAttention),
            _ => Err(ParseStatusError),
        }
    }
}

/// A StatusNotifierItem (SNI).
pub struct Item {
    pub sni: proxy::StatusNotifierItemProxy<'static>,
}

impl Item {
    /// Build from a Watcher-format address `{bus}{object_path}`.
    pub async fn from_address(con: &zbus::Connection, service: &str) -> zbus::Result<Self> {
        let (addr, path) = if let Some((addr, path)) = service.split_once('/') {
            (addr.to_owned(), format!("/{}", path))
        } else if service.starts_with(':') {
            (
                service.to_owned(),
                resolve_pathless_address(con, service, "/".to_owned())
                    .await?
                    .ok_or_else(|| {
                        zbus::Error::Failure(format!("no StatusNotifierItem found for {service}"))
                    })?,
            )
        } else {
            return Err(zbus::Error::Address(service.to_owned()));
        };

        let sni = proxy::StatusNotifierItemProxy::builder(con)
            .destination(addr)?
            .path(path)?
            .build()
            .await?;

        Ok(Self { sni })
    }

    pub async fn status(&self) -> zbus::Result<Status> {
        let s = self.sni.status().await?;
        s.parse()
            .map_err(|_| zbus::Error::Failure(format!("invalid status {:?}", s)))
    }

    /// Resolve icon for this item; `size` is the target pixel size (e.g. 24).
    pub async fn load_icon_result(&self, size: i32) -> IconResult {
        icon::load_icon_from_sni(&self.sni, size).await
    }
}

// ── XML introspection for pathless addresses ──────────────────────────────────

#[derive(Deserialize)]
struct DBusNode {
    #[serde(default)]
    interface: Vec<DBusInterface>,
    #[serde(default)]
    node: Vec<DBusNode>,
    #[serde(rename = "@name")]
    name: Option<String>,
}

#[derive(Deserialize)]
struct DBusInterface {
    #[serde(rename = "@name")]
    name: String,
}

async fn resolve_pathless_address(
    con: &zbus::Connection,
    service: &str,
    path: String,
) -> zbus::Result<Option<String>> {
    let xml = IntrospectableProxy::builder(con)
        .destination(service)?
        .path(path.as_str())?
        .build()
        .await?
        .introspect()
        .await?;

    let node = quick_xml::de::from_str::<DBusNode>(&xml)
        .map_err(|e| zbus::Error::Failure(e.to_string()))?;

    if node
        .interface
        .iter()
        .any(|i| i.name == "org.kde.StatusNotifierItem")
    {
        return Ok(Some(path));
    }

    for child in node.node {
        if let Some(name) = child.name {
            if name == "StatusNotifierItem" {
                return Ok(Some(join_path(&path, name)));
            }
            let found = Box::pin(resolve_pathless_address(
                con,
                service,
                join_path(&path, name),
            ))
            .await?;
            if found.is_some() {
                return Ok(found);
            }
        }
    }

    Ok(None)
}

fn join_path(path: &str, name: String) -> String {
    format!("{}/{}", if path == "/" { "" } else { path }, name)
}
