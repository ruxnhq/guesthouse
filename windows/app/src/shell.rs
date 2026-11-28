//! How the window gets about, which has nothing of Windows in it, so its tests run on any system:
//! the sections of a machine's page, the layouts the window's width chooses between, and the keys
//! that go back.

// Off Windows only the tests use these.
#![cfg_attr(not(windows), allow(dead_code))]

use guesthouse_control::System;

/// The part of a machine's page that shows, chosen in the bar under the title bar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    /// Where it stands, how to reach it, and its log.
    Overview,
    /// Viewing a Windows machine from other devices. An Ubuntu machine has none.
    RemoteView,
    /// Memory, processors, card, TPM and nested virtualization.
    Hardware,
    /// Its network, what it connects through, and port forwards.
    Network,
    /// Its disk, where its disks are kept, shared folders and snapshots.
    Storage,
}

impl Section {
    pub const ALL: [Section; 5] =
        [Section::Overview, Section::RemoteView, Section::Hardware, Section::Network, Section::Storage];

    /// The section's name, in its bar and as its heading.
    pub fn label(self) -> &'static str {
        match self {
            Section::Overview => "Overview",
            Section::RemoteView => crate::remote_view::proposed::TITLE,
            Section::Hardware => "Hardware",
            Section::Network => "Network",
            Section::Storage => "Storage",
        }
    }

    pub fn of_label(label: &str) -> Option<Section> {
        Section::ALL.into_iter().find(|section| section.label() == label)
    }

    /// Whether a machine of `system` has this section: Remote View is a Windows guest's.
    pub fn offered(self, system: System) -> bool {
        self != Section::RemoteView || system == System::Windows
    }

    /// Whether the section holds settings, which are saved together whichever section shows.
    pub fn has_settings(self) -> bool {
        matches!(self, Section::Hardware | Section::Network | Section::Storage)
    }
}

/// How the window's width lays the pages out, which changes only at a few widths, so a window
/// being resized is drawn again only when it crosses one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Layout {
    /// How many machines' cards Home has across.
    pub columns: usize,
    /// Wide enough for two cards side by side on a machine's page.
    pub wide: bool,
    /// Too narrow for the title bar to have room for more than the machine's name and actions.
    pub narrow: bool,
}

impl Layout {
    /// The least width of a machine's card on Home, and the gap between cards.
    const CARD: f64 = 300.0;
    const GAP: f64 = 18.0;
    /// The width of Home's margins, both sides.
    const MARGINS: f64 = 80.0;

    /// The layout of a window `width` wide, in WinUI's units.
    pub fn of(width: f64) -> Layout {
        let room = (width - Layout::MARGINS + Layout::GAP).max(0.0);
        let columns = ((room / (Layout::CARD + Layout::GAP)).floor() as usize).clamp(1, 6);
        Layout { columns, wide: width >= 1040.0, narrow: width < 900.0 }
    }
}

/// Whether a key pressed is Back: Alt+Left, as in Windows' own apps, or the Back key some keyboards
/// and mice have. `key` is the key's virtual-key code.
pub fn is_back(key: u32, alt: bool) -> bool {
    const LEFT: u32 = 0x25;
    const BROWSER_BACK: u32 = 0xA6;
    (key == LEFT && alt) || key == BROWSER_BACK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alt_left_and_the_back_key_go_back_and_left_alone_does_not() {
        assert!(is_back(0x25, true));
        assert!(!is_back(0x25, false), "Left alone moves in a list or a box");
        assert!(is_back(0xA6, false));
        assert!(!is_back(0x27, true), "Alt+Right isn't back");
        assert!(!is_back(0x1B, false), "Escape is left to the box or list that has it");
    }

    #[test]
    fn home_has_as_many_cards_across_as_fit_from_one_to_six() {
        assert_eq!(Layout::of(760.0).columns, 2);
        assert_eq!(Layout::of(1180.0).columns, 3);
        assert_eq!(Layout::of(1920.0).columns, 5);
        assert_eq!(Layout::of(100.0).columns, 1);
        assert_eq!(Layout::of(8000.0).columns, 6);
    }

    #[test]
    fn a_narrow_window_is_neither_wide_nor_a_wide_one_narrow() {
        let narrow = Layout::of(760.0);
        assert!(narrow.narrow && !narrow.wide);
        let wide = Layout::of(1180.0);
        assert!(wide.wide && !wide.narrow);
    }

    #[test]
    fn each_section_is_found_by_its_name_and_remote_view_is_a_windows_guests() {
        for section in Section::ALL {
            assert_eq!(Section::of_label(section.label()), Some(section));
        }
        assert_eq!(Section::of_label("Settings"), None);
        assert!(Section::RemoteView.offered(System::Windows));
        assert!(!Section::RemoteView.offered(System::Linux));
        assert!(Section::Storage.offered(System::Linux));
        let settings: Vec<Section> = Section::ALL.into_iter().filter(|s| s.has_settings()).collect();
        assert_eq!(settings, [Section::Hardware, Section::Network, Section::Storage]);
    }
}
