// MIT — ported from elkowar/eww crates/notifier_host/src/host.rs
use crate::*;

use zbus::export::ordered_stream::{self, OrderedStreamExt};

/// Implementors receive notifications when tray items appear and disappear.
/// Must be `Send` because `run_host` runs on the tokio thread.
pub trait Host: Send {
    fn add_item(&mut self, id: &str, item: Item);
    fn remove_item(&mut self, id: &str);
}

/// Register this DBus connection as a `StatusNotifierHost` (system tray).
///
/// Returns the well-known name acquired and a proxy to the watcher, which
/// you pass to [`run_host`].
pub async fn register_as_host(
    con: &zbus::Connection,
) -> zbus::Result<(
    zbus::names::WellKnownName<'static>,
    proxy::StatusNotifierWatcherProxy<'static>,
)> {
    let snw = proxy::StatusNotifierWatcherProxy::new(con).await?;

    let pid = std::process::id();
    let mut i = 0;
    let wellknown = loop {
        use zbus::fdo::RequestNameReply::*;
        i += 1;
        let wellknown = format!("org.freedesktop.StatusNotifierHost-{}-{}", pid, i);
        let wellknown: zbus::names::WellKnownName = wellknown
            .try_into()
            .expect("generated well-known name is invalid");

        let flags = [zbus::fdo::RequestNameFlags::DoNotQueue];
        match con
            .request_name_with_flags(&wellknown, flags.into_iter().collect())
            .await?
        {
            PrimaryOwner => break wellknown,
            Exists | AlreadyOwner => {}
            InQueue => unreachable!("DoNotQueue was set"),
        };
    };

    snw.register_status_notifier_host(&wellknown).await?;
    Ok((wellknown, snw))
}

/// Run the host forever, dispatching add/remove callbacks to `host`.
///
/// Returns only on error. Call via `tokio::spawn`.
pub async fn run_host(
    host: &mut (dyn Host + Send),
    snw: &proxy::StatusNotifierWatcherProxy<'static>,
) -> zbus::Error {
    macro_rules! try_ {
        ($e:expr) => {
            match $e {
                Ok(x) => x,
                Err(e) => return e,
            }
        };
    }

    enum ItemEvent {
        NewItem(proxy::StatusNotifierItemRegistered),
        GoneItem(proxy::StatusNotifierItemUnregistered),
    }

    let new_items = try_!(snw.receive_status_notifier_item_registered().await);
    let gone_items = try_!(snw.receive_status_notifier_item_unregistered().await);

    let mut item_names = std::collections::HashSet::new();

    for svc in try_!(snw.registered_status_notifier_items().await) {
        match Item::from_address(snw.connection(), &svc).await {
            Ok(item) => {
                item_names.insert(svc.to_owned());
                host.add_item(&svc, item);
            }
            Err(e) => {
                tracing::warn!(
                    "could not create StatusNotifierItem from {:?}: {:?}",
                    svc,
                    e
                );
            }
        }
    }

    let mut ev_stream = ordered_stream::join(
        OrderedStreamExt::map(new_items, ItemEvent::NewItem),
        OrderedStreamExt::map(gone_items, ItemEvent::GoneItem),
    );
    while let Some(ev) = ev_stream.next().await {
        match ev {
            ItemEvent::NewItem(sig) => {
                let svc = try_!(sig.args()).service;
                if item_names.contains(svc) {
                    tracing::info!("duplicate new item: {:?}", svc);
                } else {
                    match Item::from_address(snw.connection(), svc).await {
                        Ok(item) => {
                            item_names.insert(svc.to_owned());
                            host.add_item(svc, item);
                        }
                        Err(e) => {
                            tracing::warn!(
                                "could not create StatusNotifierItem from {:?}: {:?}",
                                svc,
                                e
                            );
                        }
                    }
                }
            }
            ItemEvent::GoneItem(sig) => {
                let svc = try_!(sig.args()).service;
                if item_names.remove(svc) {
                    host.remove_item(svc);
                }
            }
        }
    }

    unreachable!("StatusNotifierWatcher stopped producing events")
}
