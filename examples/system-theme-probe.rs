//! Real Tauri/WebKitGTK appearance probe, with no service or credentials.
//! Run normally while switching Plasma palettes, or use --self-test with
//! GSETTINGS_BACKEND=memory for a deterministic native integration regression.
#[cfg(target_os = "linux")]
#[path = "../src/system_theme.rs"]
mod system_theme;

#[cfg(target_os = "linux")]
fn main() {
    use gtk::prelude::*;
    use std::sync::{Arc, Mutex};
    use tauri::{Manager as _, WebviewUrl, WebviewWindowBuilder};

    let testing = std::env::args().any(|arg| arg == "--self-test");
    if testing && std::env::var("GSETTINGS_BACKEND").as_deref() != Ok("memory") {
        eprintln!("--self-test requires GSETTINGS_BACKEND=memory; real desktop preferences will not be changed");
        std::process::exit(2);
    }
    let stage = Arc::new(Mutex::new(0_usize));
    let html = format!(
        r#"<!doctype html><html data-theme="system"><head><style>{}</style></head><body>
        <script>
        function report() {{
          document.title = 'theme-probe:' + JSON.stringify({{
            appearance: document.documentElement.dataset.systemTheme,
            preference: document.documentElement.dataset.theme,
            background: getComputedStyle(document.body).backgroundColor,
            media: matchMedia('(prefers-color-scheme: dark)').matches
          }});
        }}
        window.addEventListener('mihomo-system-theme-changed', report);
        report();
        </script></body></html>"#,
        include_str!("../ui/setup.css")
    );
    // Use a private loopback fixture so the probe needs neither a running
    // management service nor the optional Tauri data-URL feature.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("probe HTTP listener");
    let url = format!("http://{}/", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        use std::io::{Read as _, Write as _};
        for mut stream in listener.incoming().flatten() {
            let mut request = [0_u8; 8192];
            let _ = stream.read(&mut request);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                html.len(),
                html
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    let app = tauri::Builder::default()
        .manage(system_theme::SystemTheme::default())
        .on_window_event(system_theme::window_event)
        .setup(move |app| {
            if testing {
                let preference = gtk::gio::Settings::new("org.gnome.desktop.interface");
                preference.set_string("color-scheme", "prefer-dark")?;
            }
            system_theme::observe(app.handle());
            WebviewWindowBuilder::new(app, "theme-probe", WebviewUrl::External(url.parse()?))
                .title("System theme probe")
                .visible(!testing)
                .initialization_script(system_theme::initialization_script(app.handle()))
                .on_page_load(|window, _| system_theme::sync(&window))
                .on_document_title_changed(move |window, title| {
                    let Some(json) = title.strip_prefix("theme-probe:") else {
                        return;
                    };
                    println!("THEME_PROBE {json}");
                    if !testing {
                        return;
                    }
                    let data: serde_json::Value = serde_json::from_str(json).expect("probe report");
                    let mut stage = stage.lock().unwrap();
                    let expected = [
                        ("dark", "system", "rgb(22, 24, 29)"),
                        ("light", "system", "rgb(247, 248, 250)"),
                        ("dark", "system", "rgb(22, 24, 29)"),
                        ("dark", "light", "rgb(247, 248, 250)"),
                        ("light", "light", "rgb(247, 248, 250)"),
                        ("light", "dark", "rgb(22, 24, 29)"),
                        ("light", "system", "rgb(247, 248, 250)"),
                    ];
                    let Some((appearance, preference, background)) = expected.get(*stage) else {
                        return;
                    };
                    if data["appearance"] != *appearance
                        || data["preference"] != *preference
                        || data["background"] != *background
                    {
                        eprintln!("stage {stage}: expected {appearance}/{preference}/{background}, got {json}");
                        window.app_handle().exit(1);
                        return;
                    }
                    let action = *stage;
                    *stage += 1;
                    // Schedule after the current native callback unwinds. This
                    // uses real Gio notifications, never a synthetic JS event.
                    gtk::glib::idle_add_local_once(move || match action {
                        0 | 3 => {
                            gtk::gio::Settings::new("org.gnome.desktop.interface")
                                .set_string("color-scheme", "prefer-light")
                                .unwrap();
                        }
                        1 => {
                            gtk::gio::Settings::new("org.gnome.desktop.interface")
                                .set_string("color-scheme", "prefer-dark")
                                .unwrap();
                        }
                        2 => {
                            window
                                .eval("document.documentElement.dataset.theme='light'; report();")
                                .unwrap();
                        }
                        4 => {
                            window
                                .eval("document.documentElement.dataset.theme='dark'; report();")
                                .unwrap();
                        }
                        5 => {
                            window
                                .eval("document.documentElement.dataset.theme='system'; report();")
                                .unwrap();
                        }
                        _ => {
                            println!("THEME_PROBE PASS: all seven native WebView checks passed");
                            window.app_handle().exit(0);
                        }
                    });
                })
                .build()?;
            if testing {
                let handle = app.handle().clone();
                gtk::glib::timeout_add_seconds_local_once(20, move || {
                    eprintln!("THEME_PROBE timed out waiting for a native notification/WebView update");
                    handle.exit(2);
                });
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("create theme probe");
    std::process::exit(app.run_return(|_, _| {}));
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("This native GSettings/WebKitGTK probe is Linux-only.");
}
