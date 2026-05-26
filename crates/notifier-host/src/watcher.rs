// MIT — ported from elkowar/eww crates/notifier_host/src/watcher.rs
use crate::names;
use zbus::{dbus_interface, export::ordered_stream::OrderedStreamExt, Interface};

/// An instance of `org.kde.StatusNotifierWatcher`. Tracks tray items and hosts;
/// does not render anything (see [`Host`][`crate::Host`] for that).
#[derive(Debug, Default)]
pub struct Watcher {
    tasks: tokio::task::JoinSet<()>,

    // std::sync::Mutex is intentional: we never hold across an await.
    hosts: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    items: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
}

#[dbus_interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    async fn register_status_notifier_host(
        &mut self,
        service: &str,
        #[zbus(header)] hdr: zbus::MessageHeader<'_>,
        #[zbus(connection)] con: &zbus::Connection,
        #[zbus(signal_context)] ctxt: zbus::SignalContext<'_>,
    ) -> zbus::fdo::Result<()> {
        let (service, _) = parse_service(service, hdr, con).await?;
        tracing::info!("new host: {}", service);

        let added_first = {
            let mut hosts = self.hosts.lock().unwrap();
            if !hosts.insert(service.to_string()) {
                return Ok(());
            }
            hosts.len() == 1
        };

        if added_first {
            self.is_status_notifier_host_registered_changed(&ctxt).await?;
        }
        Watcher::status_notifier_host_registered(&ctxt).await?;

        self.tasks.spawn({
            let hosts = self.hosts.clone();
            let ctxt = ctxt.to_owned();
            let con = con.to_owned();
            async move {
                if let Err(e) = wait_for_service_exit(&con, service.as_ref().into()).await {
                    tracing::error!("failed to wait for service exit: {}", e);
                }
                tracing::info!("lost host: {}", service);

                let removed_last = {
                    let mut hosts = hosts.lock().unwrap();
                    let did_remove = hosts.remove(service.as_str());
                    did_remove && hosts.is_empty()
                };

                if removed_last
                    && let Err(e) = Watcher::is_status_notifier_host_registered_refresh(&ctxt).await {
                    tracing::error!("failed to signal Watcher: {}", e);
                }
                if let Err(e) = Watcher::status_notifier_host_unregistered(&ctxt).await {
                    tracing::error!("failed to signal Watcher: {}", e);
                }
            }
        });

        Ok(())
    }

    #[dbus_interface(signal)]
    async fn status_notifier_host_registered(ctxt: &zbus::SignalContext<'_>) -> zbus::Result<()>;

    #[dbus_interface(signal)]
    async fn status_notifier_host_unregistered(ctxt: &zbus::SignalContext<'_>) -> zbus::Result<()>;

    #[dbus_interface(property)]
    async fn is_status_notifier_host_registered(&self) -> bool {
        !self.hosts.lock().unwrap().is_empty()
    }

    async fn register_status_notifier_item(
        &mut self,
        service: &str,
        #[zbus(header)] hdr: zbus::MessageHeader<'_>,
        #[zbus(connection)] con: &zbus::Connection,
        #[zbus(signal_context)] ctxt: zbus::SignalContext<'_>,
    ) -> zbus::fdo::Result<()> {
        let (service, objpath) = parse_service(service, hdr, con).await?;
        let service = zbus::names::BusName::Unique(service);
        let item = format!("{}{}", service, objpath);

        {
            let mut items = self.items.lock().unwrap();
            if !items.insert(item.clone()) {
                tracing::info!("new item: {} (duplicate)", item);
                return Ok(());
            }
        }
        tracing::info!("new item: {}", item);

        self.registered_status_notifier_items_changed(&ctxt).await?;
        Watcher::status_notifier_item_registered(&ctxt, item.as_ref()).await?;

        self.tasks.spawn({
            let items = self.items.clone();
            let ctxt = ctxt.to_owned();
            let con = con.to_owned();
            async move {
                if let Err(e) = wait_for_service_exit(&con, service.as_ref()).await {
                    tracing::error!("failed to wait for service exit: {}", e);
                }
                tracing::info!("gone item: {}", &item);

                {
                    let mut items = items.lock().unwrap();
                    items.remove(&item);
                }

                if let Err(e) = Watcher::registered_status_notifier_items_refresh(&ctxt).await {
                    tracing::error!("failed to signal Watcher: {}", e);
                }
                if let Err(e) = Watcher::status_notifier_item_unregistered(&ctxt, item.as_ref()).await {
                    tracing::error!("failed to signal Watcher: {}", e);
                }
            }
        });

        Ok(())
    }

    #[dbus_interface(signal)]
    async fn status_notifier_item_registered(ctxt: &zbus::SignalContext<'_>, service: &str) -> zbus::Result<()>;

    #[dbus_interface(signal)]
    async fn status_notifier_item_unregistered(ctxt: &zbus::SignalContext<'_>, service: &str) -> zbus::Result<()>;

    #[dbus_interface(property)]
    async fn registered_status_notifier_items(&self) -> Vec<String> {
        self.items.lock().unwrap().iter().cloned().collect()
    }

    #[dbus_interface(property)]
    fn protocol_version(&self) -> i32 {
        0
    }
}

impl Watcher {
    pub fn new() -> Watcher {
        Default::default()
    }

    pub async fn attach_to(self, con: &zbus::Connection) -> zbus::Result<()> {
        if !con.object_server().at(names::WATCHER_OBJECT, self).await? {
            return Err(zbus::Error::Failure(format!(
                "Object already exists at {} — is StatusNotifierWatcher already running?",
                names::WATCHER_OBJECT
            )));
        }

        let flags: [zbus::fdo::RequestNameFlags; 0] = [];
        match con.request_name_with_flags(names::WATCHER_BUS, flags.into_iter().collect()).await {
            Ok(zbus::fdo::RequestNameReply::PrimaryOwner) => Ok(()),
            Ok(_) | Err(zbus::Error::NameTaken) => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn is_status_notifier_host_registered_refresh(ctxt: &zbus::SignalContext<'_>) -> zbus::Result<()> {
        zbus::fdo::Properties::properties_changed(
            ctxt,
            Self::name(),
            &std::collections::HashMap::new(),
            &["IsStatusNotifierHostRegistered"],
        )
        .await
    }

    async fn registered_status_notifier_items_refresh(ctxt: &zbus::SignalContext<'_>) -> zbus::Result<()> {
        zbus::fdo::Properties::properties_changed(
            ctxt,
            Self::name(),
            &std::collections::HashMap::new(),
            &["RegisteredStatusNotifierItems"],
        )
        .await
    }
}

async fn parse_service<'a>(
    service: &'a str,
    hdr: zbus::MessageHeader<'_>,
    con: &zbus::Connection,
) -> zbus::fdo::Result<(zbus::names::UniqueName<'static>, &'a str)> {
    if service.starts_with('/') {
        if let Some(sender) = hdr.sender()? {
            Ok((sender.to_owned(), service))
        } else {
            tracing::warn!("unknown sender");
            Err(zbus::fdo::Error::InvalidArgs("Unknown bus address".into()))
        }
    } else {
        let busname: zbus::names::BusName = match service.try_into() {
            Ok(x) => x,
            Err(e) => {
                tracing::warn!("received invalid bus name {:?}: {}", service, e);
                return Err(zbus::fdo::Error::InvalidArgs(e.to_string()));
            }
        };

        if let zbus::names::BusName::Unique(unique) = busname {
            Ok((unique.to_owned(), names::ITEM_OBJECT))
        } else {
            let dbus = zbus::fdo::DBusProxy::new(con).await?;
            match dbus.get_name_owner(busname).await {
                Ok(owner) => Ok((owner.into_inner(), names::ITEM_OBJECT)),
                Err(e) => {
                    tracing::warn!("failed to get owner of {:?}: {}", service, e);
                    Err(e)
                }
            }
        }
    }
}

async fn wait_for_service_exit(con: &zbus::Connection, service: zbus::names::BusName<'_>) -> zbus::fdo::Result<()> {
    let dbus = zbus::fdo::DBusProxy::new(con).await?;
    let mut owner_changes = dbus.receive_name_owner_changed_with_args(&[(0, &service)]).await?;

    if !dbus.name_has_owner(service.as_ref()).await? {
        return Ok(());
    }

    while let Some(sig) = owner_changes.next().await {
        let args = sig.args()?;
        if args.new_owner().is_none() {
            break;
        }
    }

    Ok(())
}
