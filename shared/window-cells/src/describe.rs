//! One cell as one line for a person, for `guesthouse-vm windows`.

use super::message::{Cell, State};

/// A title can hold anything, line breaks included, so it is written the way a program writes a
/// string. Where a popup's picture is differs from where the window is, and that is said.
pub fn describe(cell: &Cell) -> String {
    let exe = if cell.exe.is_empty() { "(unknown program)" } else { &cell.exe };
    let frame = cell.frame;
    let mut notes = Vec::new();
    if cell.focus {
        notes.push("focus".to_owned());
    }
    match cell.state {
        State::Shown => {}
        State::Minimized => notes.push("minimized".to_owned()),
        State::Covered => notes.push("covered".to_owned()),
    }
    if cell.overflow {
        notes.push("overflow".to_owned());
    }
    if cell.popup {
        notes.push(format!("popup of {} at {},{}", cell.parent, cell.rel.x, cell.rel.y));
    }
    if cell.cell != cell.frame {
        let picture = cell.cell;
        notes.push(format!("picture {}x{} at {},{}", picture.w, picture.h, picture.x, picture.y));
    }
    let notes = if notes.is_empty() { String::new() } else { format!(" [{}]", notes.join(", ")) };
    format!("{} {exe} {:?} {}x{} at {},{}{notes}", cell.id, cell.title, frame.w, frame.h, frame.x, frame.y)
}
