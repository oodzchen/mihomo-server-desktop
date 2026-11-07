//! Forward native appearance changes instead of relying only on WebKitGTK's
//! sometimes-stale prefers-color-scheme. Never force the native window theme.
use std::sync::Mutex;
use tauri::{AppHandle, Manager as _, Theme, WebviewWindow, Window, WindowEvent};

const BRIDGE: &str = include_str!("../ui/system-theme.js");

#[derive(Clone, Copy)]
struct Appearance {
    theme: Theme,
    // Tauri's Linux ThemeChanged comes from the appearance portal. Once seen,
    // it takes precedence over GTK theme names (which can remain dark).
    native: bool,
}

#[derive(Default)]
pub struct SystemTheme(Mutex<Option<Appearance>>);

fn name(theme: Theme) -> &'static str {
    if theme == Theme::Dark { "dark" } else { "light" }
}

fn update_script(theme: Theme) -> String {
    format!("window.__MIHOMO_SYSTEM_THEME__?.update({:?});", name(theme))
}

pub fn initialization_script(app: &AppHandle) -> String {
    let current = *app
        .state::<SystemTheme>()
        .0
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut script = BRIDGE.to_owned();
    if let Some(current) = current {
        script.push_str(&update_script(current.theme));
    }
    script
}

fn publish(app: &AppHandle, appearance: Appearance) {
    *app.state::<SystemTheme>()
        .0
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(appearance);
    let script = update_script(appearance.theme);
    for window in app.webview_windows().values() {
        if let Err(error) = window.eval(&script) {
            eprintln!("cannot forward the system theme: {error}");
        }
    }
}

/// Callers include async commands and the connection monitor, not just native
/// callbacks. GTK must only be accessed on the event-loop thread.
pub fn sync(window: &WebviewWindow) {
    let target = window.clone();
    let _ = window.run_on_main_thread(move || sync_on_main_thread(&target));
}

/// Reapply after navigation or restoring a hidden window. Native events are
/// cached even when a WebView is suspended; no blocking portal read is needed.
fn sync_on_main_thread(window: &WebviewWindow) {
    let current = *window
        .state::<SystemTheme>()
        .0
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(mut current) = current else { return };
    #[cfg(target_os = "linux")]
    if !current.native
        && let Some(settings) = gtk::Settings::default()
    {
        current.theme = gtk_theme(&settings);
    }
    publish(window.app_handle(), current);
}

pub fn window_event(window: &Window, event: &WindowEvent) {
    match event {
        WindowEvent::ThemeChanged(theme) => publish(
            window.app_handle(),
            Appearance {
                theme: *theme,
                native: true,
            },
        ),
        WindowEvent::Focused(true) => {
            if let Some(webview) = window.app_handle().get_webview_window(window.label()) {
                sync(&webview);
            }
        }
        _ => {}
    }
}

#[cfg(target_os = "linux")]
fn gtk_theme(settings: &gtk::Settings) -> Theme {
    use gtk::prelude::*;
    gtk_appearance(
        settings.is_gtk_application_prefer_dark_theme(),
        settings.gtk_theme_name().as_deref(),
    )
}

#[cfg(target_os = "linux")]
fn gtk_appearance(prefer_dark: bool, theme_name: Option<&str>) -> Theme {
    if prefer_dark || theme_name.is_some_and(|name| name.to_ascii_lowercase().contains("dark")) {
        Theme::Dark
    } else {
        Theme::Light
    }
}

/// GTK notifications also work on desktops without an XDG appearance portal.
#[cfg(target_os = "linux")]
pub fn observe(app: &AppHandle) {
    use gtk::prelude::*;
    let Some(settings) = gtk::Settings::default() else {
        return;
    };
    for property in ["gtk-application-prefer-dark-theme", "gtk-theme-name"] {
        let app = app.clone();
        settings.connect_notify_local(Some(property), move |settings, _| {
            let current = *app
                .state::<SystemTheme>()
                .0
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            // Portal changes are forwarded by Tauri, after it updates GTK. Do
            // not let a GTK theme-name notification override that preference.
            if !current.is_some_and(|current| current.native) {
                publish(
                    &app,
                    Appearance {
                        theme: gtk_theme(settings),
                        native: false,
                    },
                );
            }
        });
    }
}

#[cfg(not(target_os = "linux"))]
pub fn observe(_app: &AppHandle) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_updates_only_contain_a_valid_appearance() {
        assert_eq!(
            update_script(Theme::Dark),
            "window.__MIHOMO_SYSTEM_THEME__?.update(\"dark\");"
        );
        assert_eq!(
            update_script(Theme::Light),
            "window.__MIHOMO_SYSTEM_THEME__?.update(\"light\");"
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn gtk_fallback_tracks_both_dark_preference_and_named_themes() {
        assert_eq!(gtk_appearance(true, Some("Breeze")), Theme::Dark);
        assert_eq!(gtk_appearance(false, Some("Breeze-Dark")), Theme::Dark);
        assert_eq!(gtk_appearance(false, Some("Adwaita")), Theme::Light);
        assert_eq!(gtk_appearance(false, None), Theme::Light);
    }
}
