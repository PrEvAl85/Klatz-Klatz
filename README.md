# Klatz-Klatz — Keyboard Layout Switcher

[English](README.md) | [Русский](README.ru.md)

A cross-platform tray application that switches the keyboard layout with a global hotkey or a single
click on the tray icon. Built with Rust + Tauri 2, installs as one file, works on **Windows**,
**Linux (X11)** and **macOS**.

It does one thing and does it locally: no account, no telemetry, no network requests, no automatic
updates. Settings are a single JSON file next to your other app configs.

## Features

- **Global hotkey** — switch layouts with `Ctrl+Shift+Space` (configurable, any accelerator is
  accepted).
- **Tray icon** — click to switch, right-click for a menu.
- **Tray menu** — a list of the installed layouts with the active one marked.
- **Custom cycle order** — reorder the layouts by dragging them in the settings window; the hotkey
  walks the cycle in exactly that order.
- **Click-to-switch option** — turn tray clicks into no-ops if you only want the hotkey.
- **Autostart** — start with the system, configurable from the settings window.
- **Settings window hides to the tray** — closing it does not quit the app.
- **Busy hotkey is reported, not fatal** — if another app already owns `Ctrl+Shift+Space`,
  Klatz-Klatz keeps running and shows the problem in the UI instead of refusing to start.
- **Zero-install footprint** — no service, no background daemon, no admin rights.

## Automatic switching by language

Optional, off by default, Windows only. Type the beginning of an English word while the Russian
layout is active and Klatz-Klatz notices that the text cannot be Russian, switches to the English
layout and **fixes the letters it already got wrong** — the word you meant appears, instead of a
row of Cyrillic that has to be deleted by hand.

It works by looking at the same physical keys rather than at the letters: on a Russian layout the
English word `Today` arrives as `ещвфн`, and those keys read `today` on a US layout. The layout
table is built at run time from the layouts you have installed, so AZERTY, QWERTZ, Dvorak and any
other layout are handled without a lookup table in the source.

**Languages** are read from the layouts themselves — the layout id already carries the locale, so
nothing has to be assigned by hand. Word lists ship for 29 languages:

| Script | Languages |
| --- | --- |
| Cyrillic | Russian, Ukrainian, Bulgarian, Serbian, Slovenian |
| Western European | English, German, French, Spanish, Italian, Portuguese, Dutch, Catalan, Galician |
| Central and Eastern Europe | Polish, Czech, Slovak, Hungarian, Romanian, Turkish |
| Nordic | Swedish, Danish, Norwegian, Finnish, Icelandic |
| Greek | Greek |
| South-East Asia | Indonesian, Vietnamese |

Detection is at its best between alphabets that do not share letters, which in practice means
Russian ↔ English. Between two Latin languages there is more to be ambiguous about, so it needs
slightly longer words before it acts.

### Password fields

Nothing is typed into a password field, and nothing typed there is inspected:

- Klatz-Klatz asks the system whether the focused element is a password box, and stays silent when
  the system cannot answer — a question it cannot answer is treated as "yes, it is a password".
- Password managers are excluded by default; the list is editable in the settings.
- The window that answers is remembered until the focus moves, so the check costs nothing per
  keystroke.
- The text itself never leaves the buffer of the current word: it is never written to disk, never
  sent anywhere, and forgotten at the end of the word.

Windows' own secure desktop (the UAC prompt) never hands keys to a global hook in the first place,
so automatic switching cannot run there at all.

## Installation

Download the build for your platform from the [Releases](https://github.com/PrEvAl85/Klatz-Klatz/releases) page.

**v0.2.0 ships the Windows installer only.** macOS and Linux builds are produced by the same
workflow and appear in later releases; see [platform limitations](#platform-limitations).

| Platform | File |
|-----------|------|
| Windows | `Klatz-Klatz_0.2.0_x64-setup.exe` (NSIS) or `Klatz-Klatz_0.2.0_x64_en-US.msi` |
| macOS (Apple Silicon) | `Klatz-Klatz_0.2.0_aarch64.dmg` |
| macOS (Intel) | `Klatz-Klatz_0.2.0_x64.dmg` |
| Linux | `Klatz-Klatz_0.2.0_amd64.AppImage`, `Klatz-Klatz_0.2.0_amd64.deb` |

> **Windows SmartScreen (unsigned EXE).** The Windows builds are not signed with a digital
> certificate (code signing is paid), so Windows may show "Windows protected your PC" on first
> launch. This is expected — the file is safe:
>
> - click **"More info" → "Run anyway"** (one-time);
> - or remove the "downloaded from the internet" mark: right-click the file → **Properties** →
>   check **"Unblock"** → OK;
> - or run `Unblock-File Klatz-Klatz_0.2.0_x64-setup.exe` in PowerShell.

> **macOS.** The first launch may require permission to control the keyboard. If layouts do not
> switch, check **System Settings → Privacy & Security → Accessibility**.

## Platform limitations

These are operating-system rules, not bugs in the app:

- **Linux / Wayland** — Wayland does not allow an ordinary application to change the layout.
  Klatz-Klatz detects such a session and warns you instead of silently doing nothing. Only X11
  (Xorg) is supported.
- **macOS** — the system may require Accessibility permission; see the note above.
- **Windows** — the layout can only be changed in the focused window, so there must be an active
  window at the moment of the switch.

## How it works

Switching a layout is done completely differently on every OS, so all system-specific code lives
in `src-tauri/src/platform/` and the rest of the app talks to a single shared data model.

| Platform | Mechanism |
| --- | --- |
| Windows | `GetKeyboardLayoutList` to enumerate, `PostMessageW` with `WM_INPUTLANGCHANGEREQUEST` to the foreground window |
| Linux | Parse `setxkbmap -query`, reorder the layout groups, call `setxkbmap` again |
| macOS | `TISCreateInputSourceList` / `TISSelectInputSource` from Carbon HIToolbox |

On Windows a layout is identified by its `HKL` in hex (for example `04090419`), because different
keyboard layouts can share the same locale — US and Dvorak, for instance.

## Build & Run

Requirements: [Rust](https://rustup.rs) (stable), [Node.js](https://nodejs.org) ≥ 20, Tauri
[system dependencies](https://tauri.app/start/prerequisites/).

```sh
npm install
npm run tauri dev     # development
npm run tauri build   # release build (msi/nsis/dmg/appimage/…)
```

System dependencies on Debian/Ubuntu:

```sh
sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf
```

## Quality checks

```sh
cd src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Test coverage: the layout cycle (forward, wrap-around, single element, unknown current id, empty
cycle), moving a layout to the front of the cycle, and config handling (missing fields fall back to
defaults, stale layout ids are dropped while the order is kept, a corrupt file falls back to
defaults).

The same checks run on every push to `main` and on every pull request (GitHub Actions).

## Settings

Stored next to your other app configs:

- Windows — `%APPDATA%\com.klatz.klatz\config.json`
- Linux — `~/.config/com.klatz.klatz/config.json`
- macOS — `~/Library/Application Support/com.klatz.klatz/config.json`

```json
{
  "layouts": ["04090419", "04190419"],
  "hotkey": "CmdOrCtrl+Shift+Space",
  "switchOnClick": true,
  "autostart": false,
  "autoSwitch": false,
  "autoSwitchExcludedApps": ["1password.exe", "keepassxc.exe"]
}
```

`layouts` is the cycle order exactly as you arranged it in the window, not a copy of the system
list, so your custom order survives restarts. `autoSwitchExcludedApps` is replaced wholesale when
you save it from the settings window, and the list that ships with the app is a starting point
rather than a fixed list.

## Technical details

- **Stack:** Tauri 2 (Rust) + web UI (TypeScript, Vite, no UI framework).
- **Plugins:** `tauri-plugin-global-shortcut` (hotkey), `tauri-plugin-autostart` (startup).
- **Storage:** local JSON, written atomically; nothing ever leaves the computer.
- **Security:** strict CSP, no remote content, no shell-out except `setxkbmap` on Linux X11.
- **Word lists:** `src-tauri/resources/lexicon/*.txt`, 10 000 words per language, generated from a
  public corpus. They are data files rather than compiled-in tables, and their provenance is
  recorded in [SOURCES.md](src-tauri/resources/lexicon/SOURCES.md).

## Project structure

```
src/                      settings window (TypeScript, no frameworks)
src-tauri/
  src/lib.rs              tray, global hotkey, commands for the UI
  src/config.rs           settings and their atomic save
  src/layouts.rs          layout model and cycle logic (covered by tests)
  src/autoswitch/         deciding on its own when the layout has to change
    mod.rs                the detector: text in, decision out
    lexicon.rs            word lists and what they say about a fragment
    keymap.rs             the same physical key on another layout
    windows.rs            hook, character reading, password fields, repair
    supervisor.rs         the loop that ties the above together
  src/platform/           windows.rs / linux.rs / macos.rs
  resources/lexicon/      word lists, 29 languages
tools/fetch-lexicons.ps1  regenerates the word lists
```

---

## Support the project

This project is created and maintained in free time. If Klatz-Klatz is useful to you — help its
development:

- ⭐ **Star on GitHub** — [PrEvAl85/Klatz-Klatz](https://github.com/PrEvAl85/Klatz-Klatz)
- 🐛 **Bug reports and ideas** — [Issues](https://github.com/PrEvAl85/Klatz-Klatz/issues)
- 💬 **Share** — tell others who might find it useful

**Financial support:**

- ☕ **Boosty** — https://boosty.to/pws/donate
- 🍩 **DonationAlerts** — https://www.donationalerts.com/r/photowithoutstudio

**Cryptocurrency:**

- USDT (TRC20): `TRcWS42MhyFRGdGSc6LqTH8CdTy6pLUMn6`
- USDT (BEP20): `0x0905134db34d8d54abf5b60a55406821ed7b8de0`
- BTC: `17hDrZL62DBpTjK6xNCGFFG682jN9PiVF1`
- TON: `UQCzoPJlYLHSoFGmRyh_-_ox1nOMCzx3LwG79xPR5pbjs3Aq`
- USDT in the TON network (jettons): `UQDCLQJ6fFEV7NpCJJULiqYKDd0wY5m75qC0mWwOXQvPCP23`

Thank you for using Klatz-Klatz!

---

## License

The project is distributed under the **MIT License** — a permissive free-software license. You are
free to use, copy, modify, merge, publish, distribute, sublicense, and sell copies of the software,
as well as to permit others to do the same, provided that the copyright notice below is included in
all copies or substantial portions of the software.

The software is provided "as is", without warranty of any kind, express or implied, including but
not limited to the warranties of merchantability, fitness for a particular purpose and
noninfringement. In no event shall the authors be liable for any claim, damages or other liability
arising from, out of or in connection with the software or its use.

The full license text is in the [LICENSE](LICENSE) file.