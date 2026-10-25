use crate::engine::Cmd;
use crate::MainWindow;
use slint::{ComponentHandle, Weak};
use std::sync::mpsc::Sender;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

pub fn show_window(ui: &MainWindow, tx: &Sender<Cmd>) {
    let _ = ui.show();
    ui.window().set_minimized(false);
    let _ = tx.send(Cmd::Visible(true));
}

pub fn hide_window(ui: &MainWindow, tx: &Sender<Cmd>) {
    let _ = ui.hide();
    let _ = tx.send(Cmd::Visible(false));
}

fn toggle_window(ui: &MainWindow, tx: &Sender<Cmd>) {
    if ui.window().is_visible() {
        hide_window(ui, tx);
    } else {
        show_window(ui, tx);
    }
}

fn icon() -> Option<Icon> {
    let img = image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png).ok()?;
    let small = img.resize_exact(32, 32, image::imageops::FilterType::Triangle).to_rgba8();
    Icon::from_rgba(small.into_raw(), 32, 32).ok()
}

/// Creates the tray icon. Keep the returned value alive: dropping it removes the icon.
pub fn create(ui: Weak<MainWindow>, tx: Sender<Cmd>) -> Option<TrayIcon> {
    let toggle = MenuItem::new("Play / pause", true, None);
    let next = MenuItem::new("Next", true, None);
    let prev = MenuItem::new("Previous", true, None);
    let show = MenuItem::new("Show window", true, None);
    let quit = MenuItem::new("Quit", true, None);
    let menu = Menu::new();
    menu.append_items(&[&toggle, &next, &prev, &PredefinedMenuItem::separator(), &show, &quit]).ok()?;

    let (toggle_id, next_id, prev_id, show_id, quit_id) =
        (toggle.id().clone(), next.id().clone(), prev.id().clone(), show.id().clone(), quit.id().clone());
    let (ui_menu, tx_menu) = (ui.clone(), tx.clone());
    MenuEvent::set_event_handler(Some(move |ev: MenuEvent| {
        if ev.id == toggle_id {
            let _ = tx_menu.send(Cmd::TogglePause);
        } else if ev.id == next_id {
            let _ = tx_menu.send(Cmd::Next);
        } else if ev.id == prev_id {
            let _ = tx_menu.send(Cmd::Prev);
        } else if ev.id == show_id {
            let tx = tx_menu.clone();
            let _ = ui_menu.upgrade_in_event_loop(move |ui| show_window(&ui, &tx));
        } else if ev.id == quit_id {
            let _ = tx_menu.send(Cmd::Quit);
        }
    }));

    TrayIconEvent::set_event_handler(Some(move |ev: TrayIconEvent| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
            let tx = tx.clone();
            let _ = ui.upgrade_in_event_loop(move |ui| toggle_window(&ui, &tx));
        }
    }));

    TrayIconBuilder::new()
        .with_icon(icon()?)
        .with_tooltip("ytmusic-lite")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .ok()
}
