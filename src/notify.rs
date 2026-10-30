//! Desktop notifications (freedesktop D-Bus) for failures and for a TUN another
//! account took over: the tray menu stays short and a tooltip is only seen on
//! hover, so these could go unnoticed.
use std::collections::HashMap;
use zbus::zvariant::Value;

pub async fn failure(summary: &str, body: &str) {
    show(summary, body).await;
}

/// Events the user should notice although the menu changes by itself.
pub async fn show(summary: &str, body: &str) {
    let hints: HashMap<&str, Value<'_>> = HashMap::new();
    let sent = async {
        zbus::Connection::session()
            .await?
            .call_method(
                Some("org.freedesktop.Notifications"),
                "/org/freedesktop/Notifications",
                Some("org.freedesktop.Notifications"),
                "Notify",
                // app name, replaces id, icon, summary, body, actions, hints, timeout
                &(
                    "Mihomo Server",
                    0u32,
                    "mihomo-server-desktop",
                    summary,
                    body,
                    Vec::<&str>::new(),
                    hints,
                    -1i32,
                ),
            )
            .await
    };
    if let Err(error) = sent.await {
        eprintln!("cannot show a notification: {error}");
    }
}
