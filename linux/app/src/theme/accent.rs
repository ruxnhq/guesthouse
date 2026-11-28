//! The accent colour: the desktop's, through the settings portal, and the colours that go with it.

use adw::prelude::*;
use gtk::gio;

/// How long the desktop gets to say its accent colour. It's asked while the app starts.
const PORTAL_WAIT_MS: i32 = 500;
/// Above this lightness an accent takes dark text on it, and below it light.
const LIGHT_ACCENT: f64 = 0.55;

/// Red, green and blue, each from 0 to 1.
pub type Rgb = (f64, f64, f64);

/// The desktop's accent colour, where it has one and a settings portal to say it through.
pub fn desktop_accent() -> Option<Rgb> {
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).ok()?;
    let asked = ("org.freedesktop.appearance", "accent-color").to_variant();
    let reply = bus
        .call_sync(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Settings",
            "ReadOne",
            Some(&asked),
            None,
            gio::DBusCallFlags::NONE,
            PORTAL_WAIT_MS,
            gio::Cancellable::NONE,
        )
        .ok()?;
    let colour = reply.child_value(0).as_variant()?.get::<Rgb>()?;
    // A desktop with no accent set answers with numbers outside the range.
    let in_range = |part: f64| (0.0..=1.0).contains(&part);
    (in_range(colour.0) && in_range(colour.1) && in_range(colour.2)).then_some(colour)
}

/// The accent's colours under libadwaita's names, to go after the palette's own.
pub fn accent_style((red, green, blue): Rgb) -> String {
    let byte = |part: f64| (part * 255.0).round() as u8;
    let colour = format!("rgb({}, {}, {})", byte(red), byte(green), byte(blue));
    // How light the eye takes it to be, which decides what can be read on it.
    let lightness = 0.2126 * red + 0.7152 * green + 0.0722 * blue;
    let on_it = if lightness > LIGHT_ACCENT { "#07080a" } else { "#ffffff" };
    format!(
        "@define-color accent_color {colour};\n@define-color accent_bg_color {colour};\n\
         @define-color accent_fg_color {on_it};\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_accent_takes_text_that_can_be_read_on_it() {
        assert!(accent_style((0.95, 0.85, 0.3)).contains("accent_fg_color #07080a"));
        assert!(accent_style((0.1, 0.2, 0.6)).contains("accent_fg_color #ffffff"));
        assert!(accent_style((1.0, 0.0, 0.5)).contains("accent_bg_color rgb(255, 0, 128)"));
    }
}
