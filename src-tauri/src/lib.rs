//! Klatz-Klatz — cross-platform keyboard layout switcher.
//!
//! The crate is split into four layers:
//!
//! - [`config`] — persisted user settings,
//! - [`layouts`] — platform-independent data model and cycle maths,
//! - [`autoswitch`] — deciding on its own when the layout has to change,
//! - [`platform`] — the only code that knows about Win32 / X11 / Carbon.

// Automatic switching is built and tested but not wired into the tray yet:
// the platform layer has to learn to observe the keyboard first.
#[allow(dead_code)]
mod autoswitch;
mod config;
mod layouts;
mod platform;

use config::Config;
use layouts::AppState;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::menu::{CheckMenuItemBuilder, Menu, MenuItemBuilder, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Wry};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

const TRAY_ID: &str = "main";
const SETTINGS_WINDOW: &str = "main";
const MENU_SETTINGS: &str = "settings";
const MENU_QUIT: &str = "quit";
const MENU_LAYOUT_PREFIX: &str = "layout:";

/// Payload of the `layout-changed` event sent to the settings window.
#[derive(Clone, serde::Serialize)]
struct LayoutChanged {
    id: Option<String>,
}

struct App {
    config_dir: PathBuf,
    /// Where the bundled word lists were unpacked.
    lexicon_dir: PathBuf,
    config: Mutex<Config>,
    /// A busy or malformed hotkey must never stop the app from running, so
    /// the failure is remembered and surfaced in the settings window instead.
    hotkey_error: Mutex<Option<String>>,
    /// Running automatic switching, if the user asked for it.
    auto_switch: Mutex<Option<autoswitch::supervisor::Supervisor>>,
    /// Why automatic switching is not running, when it should be.
    auto_switch_error: Mutex<Option<String>>,
}

impl App {
    fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    fn config(&self) -> Config {
        self.config.lock().expect("config mutex poisoned").clone()
    }

    /// Persists first: if the disk write fails the in-memory state stays
    /// untouched, so what the user sees is always what was saved.
    fn set_config(&self, config: Config) -> Result<(), String> {
        config::save(self.config_dir(), &config)?;
        *self.config.lock().expect("config mutex poisoned") = config;
        Ok(())
    }

    fn set_hotkey_error(&self, error: Option<String>) {
        *self.hotkey_error.lock().expect("hotkey mutex poisoned") = error;
    }

    /// Starts automatic switching if the settings ask for it, and stops it if
    /// they no longer do.
    ///
    /// Restarting rather than reconfiguring keeps the watcher simple: it is
    /// cheap to build and impossible to get half-way into a new state.
    fn refresh_auto_switch(&self, config: &Config) {
        let mut running = self.auto_switch.lock().expect("auto switch mutex poisoned");

        if !config.auto_switch {
            running.take();
            return;
        }
        if running.is_some() {
            return;
        }

        match self.start_auto_switch(config) {
            Ok(supervisor) => {
                eprintln!(
                    "klatz: автоматическое переключение включено ({} раскладок)",
                    autoswitch::supervisor::layouts_for(
                        &config.layouts,
                        platform::imp::language_of
                    )
                    .len()
                );
                *running = Some(supervisor);
                *self
                    .auto_switch_error
                    .lock()
                    .expect("auto switch mutex poisoned") = None;
            }
            Err(error) => {
                eprintln!("klatz: {error}");
                *self
                    .auto_switch_error
                    .lock()
                    .expect("auto switch mutex poisoned") = Some(error);
            }
        }
    }

    fn start_auto_switch(
        &self,
        config: &Config,
    ) -> Result<autoswitch::supervisor::Supervisor, String> {
        let layouts =
            autoswitch::supervisor::layouts_for(&config.layouts, platform::imp::language_of);

        autoswitch::supervisor::Supervisor::start(
            &autoswitch::supervisor::Settings {
                lexicon_dir: self.lexicon_dir.clone(),
                excluded_apps: config.auto_switch_excluded_apps.clone(),
            },
            &layouts,
        )
    }

    fn snapshot(&self) -> AppState {
        let config = self.config();
        let (supported, unsupported_reason) = platform::imp::supported();

        AppState {
            available: platform::imp::available().unwrap_or_default(),
            supported,
            unsupported_reason,
            hotkey_error: self
                .hotkey_error
                .lock()
                .expect("hotkey mutex poisoned")
                .clone(),
            auto_switch_available: autoswitch::supervisor::SUPPORTED,
            auto_switch_error: self
                .auto_switch_error
                .lock()
                .expect("auto switch mutex poisoned")
                .clone(),
            current: platform::imp::current().ok().flatten(),
            config,
        }
    }
}

// ---------------------------------------------------------------------------
// Layout switching
// ---------------------------------------------------------------------------

/// Switches to the next layout in the user's cycle and notifies the UI.
fn cycle(app: &AppHandle) -> Result<AppState, String> {
    let current = platform::imp::current().ok().flatten();
    let cycle_ids = app.state::<App>().config().layouts;

    let next = layouts::next_in_cycle(&cycle_ids, current.as_deref())
        .cloned()
        .ok_or_else(|| "В цикле нет ни одной раскладки".to_string())?;

    platform::imp::switch_to(&next)?;
    notify_layout_changed(app);
    refresh_tray(app);

    Ok(app.state::<App>().snapshot())
}

/// Switches to a specific layout, adding it to the cycle if necessary.
fn switch_to_layout(app: &AppHandle, layout_id: &str) -> Result<(), String> {
    platform::imp::switch_to(layout_id)?;

    let mut config = app.state::<App>().config();
    if !config.layouts.iter().any(|id| id == layout_id) {
        config.layouts.push(layout_id.to_string());
        app.state::<App>().set_config(config)?;
    }

    notify_layout_changed(app);
    refresh_tray(app);
    Ok(())
}

fn notify_layout_changed(app: &AppHandle) {
    let current = platform::imp::current().ok().flatten();
    let _ = app.emit("layout-changed", LayoutChanged { id: current });
}

fn open_settings(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(SETTINGS_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn quit(app: &AppHandle) {
    // Autostart entries pointing at a removed app confuse login managers.
    if app.state::<App>().config().autostart {
        let _ = app.autolaunch().disable();
    }
    app.exit(0);
}

// ---------------------------------------------------------------------------
// Tray menu
// ---------------------------------------------------------------------------

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let snapshot = app.state::<App>().snapshot();
    let menu = Menu::new(app)?;

    if snapshot.available.is_empty() {
        menu.append(
            &MenuItemBuilder::new("Раскладки не найдены")
                .enabled(false)
                .build(app)?,
        )?;
    } else {
        for layout in &snapshot.available {
            let in_cycle = snapshot.config.layouts.contains(&layout.id);
            let item = CheckMenuItemBuilder::with_id(
                format!("{MENU_LAYOUT_PREFIX}{}", layout.id),
                layout.name.clone(),
            )
            .checked(in_cycle && snapshot.current.as_deref() == Some(layout.id.as_str()))
            .enabled(snapshot.supported)
            .build(app)?;
            menu.append(&item)?;
        }
    }

    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItemBuilder::with_id(MENU_SETTINGS, "Настройки…").build(app)?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItemBuilder::with_id(MENU_QUIT, "Выход").build(app)?)?;

    Ok(menu)
}

/// Tray menu entries all funnel through here.
///
/// The handler lives on the builder rather than on the menu because the menu is
/// rebuilt (and replaced) whenever the layout list changes.
fn handle_menu_event(app: &AppHandle, id: &str) {
    match id {
        id if id.starts_with(MENU_LAYOUT_PREFIX) => {
            if let Err(err) = switch_to_layout(app, id.trim_start_matches(MENU_LAYOUT_PREFIX)) {
                eprintln!("klatz: {err}");
            }
        }
        MENU_SETTINGS => open_settings(app),
        MENU_QUIT => quit(app),
        _ => {}
    }
}

fn refresh_tray(app: &AppHandle) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };

    match build_menu(app) {
        Ok(menu) => {
            let _ = tray.set_menu(Some(menu));
        }
        Err(err) => eprintln!("klatz: не удалось обновить меню трея: {err}"),
    }

    let snapshot = app.state::<App>().snapshot();
    let tooltip = match &snapshot.current {
        Some(id) => {
            let name = snapshot
                .available
                .iter()
                .find(|layout| &layout.id == id)
                .map(|layout| layout.name.clone())
                .unwrap_or_else(|| id.clone());
            format!("Klatz-Klatz — {name}")
        }
        None => "Klatz-Klatz".to_string(),
    };
    let _ = tray.set_tooltip(Some(tooltip));
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
fn get_state(app: AppHandle) -> AppState {
    app.state::<App>().snapshot()
}

#[tauri::command]
fn set_layouts(app: AppHandle, layouts: Vec<String>) -> Result<AppState, String> {
    let known: Vec<String> = platform::imp::available()
        .unwrap_or_default()
        .into_iter()
        .map(|layout| layout.id)
        .collect();

    // Preserve the requested order but silently drop anything unknown, and
    // never hand an empty cycle to the hotkey handler.
    let mut config = app.state::<App>().config();
    config.layouts = layouts
        .into_iter()
        .filter(|id| known.contains(id))
        .collect();

    if config.layouts.is_empty() {
        config.layouts = known;
    }

    app.state::<App>().set_config(config)?;
    refresh_tray(&app);

    Ok(app.state::<App>().snapshot())
}

#[tauri::command]
fn set_hotkey(app: AppHandle, hotkey: String) -> Result<AppState, String> {
    let shortcut: Shortcut = hotkey
        .parse()
        .map_err(|_| format!("Не удалось разобрать комбинацию клавиш: {hotkey}"))?;

    let previous = app.state::<App>().config().hotkey;
    if let Err(err) = register_hotkey(&app, shortcut) {
        // Keep the previous shortcut registered so the hotkey keeps working.
        if let Ok(previous_shortcut) = previous.parse::<Shortcut>() {
            let _ = register_hotkey(&app, previous_shortcut);
        }
        app.state::<App>().set_hotkey_error(Some(err.clone()));
        return Err(err);
    }

    app.state::<App>().set_hotkey_error(None);

    let mut config = app.state::<App>().config();
    config.hotkey = hotkey;
    app.state::<App>().set_config(config)?;

    Ok(app.state::<App>().snapshot())
}

#[tauri::command]
fn set_switch_on_click(app: AppHandle, value: bool) -> Result<AppState, String> {
    let mut config = app.state::<App>().config();
    config.switch_on_click = value;
    app.state::<App>().set_config(config)?;

    Ok(app.state::<App>().snapshot())
}

#[tauri::command]
fn set_autostart(app: AppHandle, value: bool) -> Result<AppState, String> {
    let launcher = app.autolaunch();
    if value {
        launcher
            .enable()
            .map_err(|err| format!("Не удалось включить автозапуск: {err}"))?;
    } else {
        launcher
            .disable()
            .map_err(|err| format!("Не удалось отключить автозапуск: {err}"))?;
    }

    let mut config = app.state::<App>().config();
    config.autostart = value;
    app.state::<App>().set_config(config)?;

    Ok(app.state::<App>().snapshot())
}

#[tauri::command]
fn cycle_layout(app: AppHandle) -> Result<AppState, String> {
    cycle(&app)
}

/// Turns automatic switching on or off.
///
/// A failed start is not fatal and not rolled back: the preference is kept, the
/// reason is shown, and the watcher is tried again the next time the settings
/// change — a machine that had only one layout installed can grow a second.
#[tauri::command]
fn set_auto_switch(app: AppHandle, value: bool) -> Result<AppState, String> {
    let state = app.state::<App>();
    let mut config = state.config();
    config.auto_switch = value;
    state.set_config(config.clone())?;
    state.refresh_auto_switch(&config);
    refresh_tray(&app);

    Ok(state.snapshot())
}

#[tauri::command]
fn set_auto_switch_excluded_apps(app: AppHandle, apps: Vec<String>) -> Result<AppState, String> {
    let state = app.state::<App>();
    let mut config = state.config();

    // Compare on the lower-cased form, which is how the guard reads the names.
    config.auto_switch_excluded_apps = apps
        .iter()
        .map(|name| name.trim().to_lowercase())
        .filter(|name| !name.is_empty())
        .collect();

    state.set_config(config.clone())?;
    state.refresh_auto_switch(&config);

    Ok(state.snapshot())
}

// ---------------------------------------------------------------------------
// Global hotkey
// ---------------------------------------------------------------------------

fn register_hotkey(app: &AppHandle, shortcut: Shortcut) -> Result<(), String> {
    app.global_shortcut()
        .unregister_all()
        .map_err(|err| format!("Не удалось снять прежнюю горячую клавишу: {err}"))?;

    app.global_shortcut().register(shortcut).map_err(|err| {
        // The plugin's message is English and Debug-heavy; keep the detail
        // in the log and show the user one actionable sentence.
        eprintln!("klatz: hotkey registration failed: {err}");
        "Комбинация занята другой программой. Выберите другую в настройках.".to_string()
    })
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed
                        && let Err(err) = cycle(app)
                    {
                        eprintln!("klatz: {err}");
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .invoke_handler(tauri::generate_handler![
            get_state,
            set_layouts,
            set_hotkey,
            set_switch_on_click,
            set_autostart,
            cycle_layout,
            set_auto_switch,
            set_auto_switch_excluded_apps,
        ])
        .on_menu_event(|app, event| handle_menu_event(app, event.id().as_ref()))
        .setup(|app| {
            let handle = app.handle().clone();

            let config_dir = handle
                .path()
                .app_config_dir()
                .unwrap_or_else(|_| PathBuf::from("."));

            let mut config = config::load(&config_dir)
                .reconciled(&platform::imp::available().unwrap_or_default());
            // The login item registry is the source of truth, not our config.
            config.autostart = handle.autolaunch().is_enabled().unwrap_or(false);

            if let Err(err) = config::save(&config_dir, &config) {
                eprintln!("klatz: {err}");
            }

            let lexicon_dir = handle
                .path()
                .resource_dir()
                .map(|dir| dir.join("lexicon"))
                .unwrap_or_else(|_| PathBuf::from("lexicon"));

            app.manage(App {
                config_dir,
                lexicon_dir,
                config: Mutex::new(config),
                hotkey_error: Mutex::new(None),
                auto_switch: Mutex::new(None),
                auto_switch_error: Mutex::new(None),
            });

            let menu = build_menu(&handle)?;

            TrayIconBuilder::with_id(TRAY_ID)
                .icon(
                    handle
                        .default_window_icon()
                        .cloned()
                        .expect("missing default window icon"),
                )
                .menu(&menu)
                .tooltip("Klatz-Klatz")
                // On Windows and Linux the menu is bound to the right button,
                // which leaves the left button free for click-to-switch.
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| {
                    let app = tray.app_handle();
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        if app.state::<App>().config().switch_on_click {
                            if let Err(err) = cycle(app) {
                                eprintln!("klatz: {err}");
                            }
                        } else {
                            open_settings(app);
                        }
                    }
                })
                .build(&handle)?;

            // A tray app owns a window that starts hidden; closing it must not
            // terminate the process.
            if let Some(window) = handle.get_webview_window(SETTINGS_WINDOW) {
                let hide_target = window.clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = hide_target.hide();
                    }
                });
            }

            let hotkey = handle.state::<App>().config().hotkey.clone();
            match hotkey.parse::<Shortcut>() {
                Ok(shortcut) => {
                    if let Err(err) = register_hotkey(&handle, shortcut) {
                        handle.state::<App>().set_hotkey_error(Some(err));
                    }
                }
                Err(_) => handle.state::<App>().set_hotkey_error(Some(format!(
                    "Горячая клавиша «{hotkey}» из настроек не разбирается."
                ))),
            }

            refresh_tray(&handle);

            // Automatic switching is opt-in, so this only does something when
            // the user has already asked for it.
            handle
                .state::<App>()
                .refresh_auto_switch(&handle.state::<App>().config());

            // Autostart must stay invisible in the tray; a normal launch opens
            // the settings window so a first-time user sees something.
            let launched_by_login = std::env::args().any(|arg| arg == "--autostart");
            if !launched_by_login {
                open_settings(&handle);
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while running Klatz-Klatz")
        .run(|_app, event| {
            if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
                // Keep the tray alive when the last window is closed.
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_a_usable_accelerator() {
        assert_eq!(config::DEFAULT_HOTKEY, "CmdOrCtrl+Shift+Space");
        assert!(
            config::DEFAULT_HOTKEY.parse::<Shortcut>().is_ok(),
            "default hotkey must parse"
        );
    }
}
