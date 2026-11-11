//! macOS window details: the title bar is transparent and the content runs under it (the tabs
//! sit in the title row), so the traffic lights are moved down to the middle of that row.

use objc2_app_kit::{NSView, NSWindowButton};
use objc2_foundation::{NSPoint, NSRect};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Puts the close, minimize and zoom buttons with the left edge of the first at `x` and their
/// centers `center_y` points below the top of the window. AppKit lays the title bar out again
/// on resizes, so this is applied whenever the window size changes.
pub fn place_traffic_lights(window: &winit::window::Window, x: f64, center_y: f64) {
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        return;
    };
    // SAFETY: winit hands out the window's content view, alive while the window is.
    let view: &NSView = unsafe { h.ns_view.cast::<NSView>().as_ref() };
    let Some(ns_window) = view.window() else {
        return;
    };
    let buttons = [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ]
    .map(|b| ns_window.standardWindowButton(b));
    let [Some(close), Some(minimize), Some(zoom)] = buttons else {
        return;
    };
    // close → NSTitlebarView → NSTitlebarContainerView.
    // SAFETY: plain view hierarchy reads on the main thread (eframe draws on it).
    let Some(titlebar) = (unsafe { close.superview() }) else {
        return;
    };
    let Some(container) = (unsafe { titlebar.superview() }) else {
        return;
    };
    let button: NSRect = close.frame();
    let spacing = minimize.frame().origin.x - close.frame().origin.x;
    // The container spans the top `2 × center_y` points, so the buttons centered in it are
    // centered on the title row.
    let window_h = ns_window.frame().size.height;
    let bar_h = (center_y * 2.0).max(button.size.height);
    let mut bar = container.frame();
    if (bar.size.height - bar_h).abs() > 0.5 || (bar.origin.y - (window_h - bar_h)).abs() > 0.5 {
        bar.size.height = bar_h;
        bar.origin.y = window_h - bar_h;
        container.setFrame(bar);
    }
    let title_h = titlebar.frame().size.height;
    let y = ((title_h - button.size.height) / 2.0).round();
    for (i, b) in [close, minimize, zoom].iter().enumerate() {
        let origin = NSPoint::new(x + i as f64 * spacing, y);
        let f = b.frame();
        if (f.origin.x - origin.x).abs() > 0.5 || (f.origin.y - origin.y).abs() > 0.5 {
            b.setFrameOrigin(origin);
        }
    }
}
