# How the app's pages look

Every page follows these, so the app reads as one thing. The helpers named here are in `src/rows.rs` and the
styles in `data/style/own.css`.

## Words

- GNOME's capitalization, as the rest of the desktop: header capitalization for titles, headings, row
  titles and buttons ("Report a Problem", "Drives of This PC"), sentence case for subtitles, descriptions and
  any text that is a sentence. A name stays as it's written (Windows, PipeWire, NVIDIA).
- A row says what it is in its title. Its subtitle is one short line at most: the current value, or what it
  does in a few words. Anything longer goes behind an info button (`info_button`, or `explain` on a row),
  which opens it in a popover.
- A group's description is one line too, or none. Its longer explanation goes behind an info button in the
  group's header (`set_header_suffix`).
- Say what happens, not how it's built: "Only through" is better as "Send traffic only through". Words like
  slirp, passt, udmabuf go in the info text, not the title.
- Numbers carry their unit in the value, not the title: "RAM" with "16 GiB", not "RAM in GiB".

## Layout

- What most people change sits first. Switches that only matter to someone who knows why they want them go on
  an Advanced page or into an Advanced group at the end.
- A value shown at the end of a row is never cut off. If it can be long, it goes in the subtitle.
- One suggested (blue) button per view, for the thing the view is for.
- Choosing between kinds of things is a set of cards (`kind-card`), each with an icon, a name and one line, and
  a `preview-tag` for what's still a preview. Choosing a setting is a row.
- Dialogs fit what they hold without scrolling at the app's default size where they can. A dialog that has
  to scroll keeps its buttons in a bottom bar that doesn't.
- Cards and rows use the app's hairline (`@hairline`) and the 20px corner of `machine-card`.

## Looks

One setting, Look, picks who draws the app (`src/theme`). Automatic picks by what the desktop says it is:

- Tide on a tiling compositor (Hyprland, Sway, niri and the like): compact, no bar of its own when the
  window is tiled, machines down a sidebar, and the frame and sidebar see-through only where the compositor
  says it blurs. A machine's card says how it stands by the classes `running`, `attention` and `failed`
  (`src/window/weight.rs`), which Tide moves and colours. Its styles are `tide.css` on `tide-dark.css` or
  `tide-light.css`, and `glass.css` where there is blur.
- KDE on Plasma: Breeze's colours, or the user's from `kdeglobals`, KWin's button order and smaller corners
  (`kde.css`).
- System everywhere else: the desktop's GTK theme, with only the app's own widgets styled (`desktop.css`).

Nothing in any look is drawn over or behind the guest's screen. `page_shots` checks that GTK reads every
look's stylesheet whole, so a property GTK doesn't have fails there.

A build with the `preview` feature shows a machine in each state without a VM: see `src/preview.rs`.

## Checking

`page-shots.sh` draws every page in software. Look at the pictures after any change to a page, and keep a
check in `examples/page_shots` for what the page must say. `remote-view-shots.sh` draws the Network page's Remote View group in each of its states, from a fixture.
