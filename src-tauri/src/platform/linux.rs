//! Linux backend for X11 sessions.
//!
//! The active XKB layout is expressed as the *first* entry of the keyboard
//! layout group, so "switching" means moving the wanted layout to the front and
//! re-applying the group with `setxkbmap`. Shelling out keeps the crate free
//! of an X11 build dependency, which matters for cross-platform packaging.

use super::Result;
use crate::layouts::{LayoutInfo, move_to_front};
use std::process::Command;

struct Query {
    layouts: Vec<String>,
    variants: Vec<String>,
}

impl Query {
    /// XKB ids are `layout` or `layout,variant`, which keeps a Russian phonetic
    /// layout distinct from plain Russian.
    fn ids(&self) -> Vec<String> {
        self.layouts
            .iter()
            .enumerate()
            .map(|(index, layout)| {
                let variant = self.variants.get(index).map(String::as_str).unwrap_or("");
                if variant.is_empty() {
                    layout.clone()
                } else {
                    format!("{layout},{variant}")
                }
            })
            .collect()
    }
}

fn split_line(line: &str, key: &str) -> Option<String> {
    let rest = line.strip_prefix(key)?;
    Some(rest.trim().to_string())
}

fn run(args: &[&str]) -> Result<String> {
    let output = Command::new("setxkbmap")
        .args(args)
        .output()
        .map_err(|err| {
            format!(
                "Не удалось запустить setxkbmap ({err}). Установите пакет с утилитой setxkbmap."
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!(
            "setxkbmap завершился с ошибкой{}{}",
            if stderr.is_empty() { "" } else { ": " },
            stderr
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn query() -> Result<Query> {
    let stdout = run(&["-query"])?;
    let mut layouts = Vec::new();
    let mut variants = Vec::new();

    for line in stdout.lines() {
        let line = line.trim();
        if let Some(value) = split_line(line, "layout:") {
            layouts = value.split_whitespace().map(str::to_string).collect();
        } else if let Some(value) = split_line(line, "variant:") {
            // Empty comma slots are significant: `, phonetic` means the second
            // layout has a variant and the first one does not.
            variants = value.split(',').map(|v| v.trim().to_string()).collect();
        }
    }

    if layouts.is_empty() {
        return Err("Не удалось прочитать текущую раскладку из setxkbmap".into());
    }

    Ok(Query { layouts, variants })
}

/// Human readable labels from `localectl list-x11-keymaps`, when available.
///
/// The tool is not installed everywhere, so this is strictly an enhancement:
/// failing to read it falls back to the raw XKB id.
fn localized_names() -> Vec<(String, String)> {
    let Ok(output) = Command::new("localectl").arg("list-x11-keymaps").output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (name, description) = line.split_once(char::is_whitespace)?;
            Some((name.to_string(), description.trim().to_string()))
        })
        .collect()
}

fn name_for(id: &str, known: &[(String, String)]) -> String {
    let (layout, variant) = id.split_once(',').unwrap_or((id, ""));

    known
        .iter()
        .find(|(name, description)| {
            name == layout
                && (variant.is_empty()
                    || description.to_lowercase().contains(&variant.to_lowercase()))
        })
        .map(|(_, description)| {
            if variant.is_empty() {
                description.clone()
            } else {
                format!("{description} ({variant})")
            }
        })
        .unwrap_or_else(|| id.to_string())
}

/// The language a layout belongs to.
///
/// Automatic switching is not implemented on Linux yet, so the layout id is
/// never asked for a language today; the function exists so the backend keeps
/// the same shape as the others.
pub fn language_of(_id: &str) -> Option<String> {
    None
}

pub fn supported() -> (bool, Option<String>) {
    let session = std::env::var("XDG_SESSION_TYPE")
        .unwrap_or_default()
        .to_lowercase();

    if session == "wayland" || std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return (
            false,
            Some(
                "Сессия Wayland не поддерживает смену раскладки сторонним приложением. \
                 Переключайте раскладку в настройках GNOME/KDE или войдите в сессию X11 (Xorg)."
                    .into(),
            ),
        );
    }

    if std::env::var_os("DISPLAY").is_none() {
        return (
            false,
            Some("Переменная DISPLAY не задана — X11-сессия не обнаружена.".into()),
        );
    }

    (true, None)
}

pub fn available() -> Result<Vec<LayoutInfo>> {
    let ids = query()?.ids();
    let known = localized_names();

    Ok(ids
        .into_iter()
        .map(|id| LayoutInfo {
            name: name_for(&id, &known),
            id,
        })
        .collect())
}

pub fn current() -> Result<Option<String>> {
    let query = query()?;
    Ok(query.ids().first().cloned())
}

pub fn switch_to(id: &str) -> Result<()> {
    let available = query()?.ids();
    if !available.iter().any(|known| known == id) {
        return Err(format!("Раскладка {id} сейчас не подключена в системе"));
    }

    let mut order = available;
    move_to_front(&mut order, id);

    // `-variant` must have exactly as many comma separated slots as `-layout`,
    // otherwise the variant of one layout silently shifts onto the next.
    let mut layouts = Vec::with_capacity(order.len());
    let mut variants = Vec::with_capacity(order.len());
    for entry in &order {
        match entry.split_once(',') {
            Some((layout, variant)) => {
                layouts.push(layout);
                variants.push(variant);
            }
            None => {
                layouts.push(entry.as_str());
                variants.push("");
            }
        }
    }

    run(&[
        "-layout",
        &layouts.join(","),
        "-variant",
        &variants.join(","),
    ])?;

    Ok(())
}
