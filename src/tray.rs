use std::cell::Cell;
use std::rc::Rc;

use crate::platform::ForegroundWindow;
use crate::ui::MainWindow;
use slint::{ComponentHandle, Timer, TimerMode, Weak};
use tray_icon::menu::{Menu, MenuItem, MenuEvent};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

pub fn create_tray_icon(
    app: Weak<MainWindow>,
    paste_target: Rc<Cell<Option<ForegroundWindow>>>,
) -> Option<TrayIcon> {
    let size = 32u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);

    for y in 0..size {
        for x in 0..size {
            let in_mark = (8..=23).contains(&x) && (8..=11).contains(&y)
                || (8..=11).contains(&x) && (8..=23).contains(&y)
                || (8..=23).contains(&x) && (20..=23).contains(&y);

            if in_mark {
                rgba.extend_from_slice(&[255, 255, 255, 255]);
            } else {
                rgba.extend_from_slice(&[77, 124, 255, 255]);
            }
        }
    }

    let icon = match Icon::from_rgba(rgba, size, size) {
        Ok(icon) => icon,
        Err(_) => return None,
    };

    let menu = Menu::new();
    let quit = MenuItem::new("退出", true, None);
    let _ = menu.append(&quit);

    let tray = match TrayIconBuilder::new()
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("CCopy")
        .build()
    {
        Ok(tray) => tray,
        Err(_) => return None,
    };

    let app_weak = app.clone();
    let tray_timer = Timer::default();
    tray_timer.start(TimerMode::Repeated, std::time::Duration::from_millis(50), move || {
        if let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click { button, button_state, .. } = event {
                if button == MouseButton::Left && button_state == MouseButtonState::Up {
                    if let Some(app) = app_weak.upgrade() {
                        crate::platform::open_panel_for_target(&app, &paste_target);
                    }
                }
            }
        }
    });

    let app_weak = app.clone();
    let menu_timer = Timer::default();
    menu_timer.start(TimerMode::Repeated, std::time::Duration::from_millis(50), move || {
        if let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == quit.id() {
                if let Some(app) = app_weak.upgrade() {
                    app.hide().ok();
                    let _ = slint::quit_event_loop();
                }
            }
        }
    });

    let _keep = Box::leak(Box::new((tray_timer, menu_timer)));

    Some(tray)
}