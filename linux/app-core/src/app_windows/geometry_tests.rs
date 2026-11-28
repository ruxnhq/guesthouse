use super::geometry::*;
use super::message::*;

fn cell(cell: Rect, frame: Rect) -> Cell {
    Cell {
        id: "1".into(),
        pid: 1,
        exe: String::new(),
        app: String::new(),
        name: String::new(),
        cls: String::new(),
        title: String::new(),
        cell,
        frame,
        popup: false,
        parent: String::new(),
        owner: String::new(),
        rel: Point::default(),
        state: State::Shown,
        overflow: false,
        focus: false,
        caption: Caption { h: 32, buttons_w: 138 },
        resizable: true,
        corner: 0,
    }
}

const SCREEN: Size = Size { w: 1920, h: 1080 };

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

#[test]
fn a_point_of_a_parked_window_is_the_same_point_of_the_screen() {
    let window = cell(rect(300, 200, 800, 600), rect(300, 200, 800, 600));
    let at = guest_point(&window, Surface::shown(&window, 1.0), 10.0, 20.0, SCREEN);
    assert_eq!(at, Point { x: 310, y: 220 });
}

#[test]
fn a_popup_sends_its_pointer_to_its_real_place_not_to_where_its_picture_is() {
    let menu = cell(rect(1000, 100, 190, 280), rect(105, 113, 190, 280));
    let at = guest_point(&menu, Surface::shown(&menu, 1.0), 0.0, 0.0, SCREEN);
    assert_eq!(at, Point { x: 105, y: 113 });
    let at = guest_point(&menu, Surface::shown(&menu, 1.0), 7.9, 31.2, SCREEN);
    assert_eq!(at, Point { x: 112, y: 144 }, "fractions of a pixel go down");
}

#[test]
fn a_point_a_hair_outside_is_kept_in_the_frame_and_then_on_the_screen() {
    let window = cell(rect(0, 0, 800, 600), rect(100, 100, 800, 600));
    let surface = Surface::shown(&window, 1.0);
    assert_eq!(guest_point(&window, surface, -0.5, -3.0, SCREEN), Point { x: 100, y: 100 });
    assert_eq!(guest_point(&window, surface, 800.0, 600.0, SCREEN), Point { x: 899, y: 699 });
    let off = cell(rect(0, 0, 800, 600), rect(1500, 700, 800, 600));
    let surface = Surface::shown(&off, 1.0);
    assert_eq!(
        guest_point(&off, surface, 799.0, 599.0, SCREEN),
        Point { x: 1919, y: 1079 },
        "a frame that hangs over the screen has no pointer beyond it"
    );
}

#[test]
fn a_surface_that_is_not_the_cells_size_is_mapped_by_the_ratio() {
    let window = cell(rect(0, 0, 800, 600), rect(100, 100, 800, 600));
    let half = Surface { w: 400.0, h: 300.0 };
    assert_eq!(guest_point(&window, half, 200.0, 150.0, SCREEN), Point { x: 500, y: 400 });
    let wide = Surface { w: 1600.0, h: 600.0 };
    assert_eq!(guest_point(&window, wide, 1599.0, 599.0, SCREEN), Point { x: 899, y: 699 });
}

#[test]
fn a_cell_with_no_size_leaves_the_point_alone() {
    let none = cell(rect(0, 0, 0, 0), rect(-32000, -32000, 160, 28));
    let at = guest_point(&none, Surface { w: 0.0, h: 0.0 }, 5.0, 5.0, SCREEN);
    assert_eq!(at, Point { x: 0, y: 0 }, "clamped to the screen as the frame is off it");
}

#[test]
fn a_cell_on_the_screen_is_cropped_whole() {
    let crop = crop(rect(0, 41, 1200, 800), SCREEN).unwrap();
    assert_eq!((crop.source, crop.offset), (rect(0, 41, 1200, 800), Point { x: 0, y: 0 }));
}

#[test]
fn a_cell_that_hangs_over_the_screen_is_cut_and_says_where_the_rest_starts() {
    let crop = crop(rect(-50, -20, 400, 300), SCREEN).unwrap();
    assert_eq!(crop.source, rect(0, 0, 350, 280));
    assert_eq!(crop.offset, Point { x: 50, y: 20 });
    let crop = super::geometry::crop(rect(1800, 1000, 400, 300), SCREEN).unwrap();
    assert_eq!(crop.source, rect(1800, 1000, 120, 80));
    assert_eq!(crop.offset, Point { x: 0, y: 0 });
}

#[test]
fn a_cell_with_no_area_or_none_on_the_screen_has_no_crop() {
    assert_eq!(crop(rect(0, 0, 0, 0), SCREEN), None);
    assert_eq!(crop(rect(2000, 0, 100, 100), SCREEN), None);
    assert_eq!(crop(rect(-500, 0, 100, 100), SCREEN), None);
}

#[test]
fn a_popup_is_placed_by_rel_in_its_parents_surface() {
    let parent = cell(rect(0, 41, 886, 593), rect(0, 41, 886, 593));
    let menu = Cell {
        popup: true,
        rel: Point { x: 5, y: 72 },
        ..cell(rect(900, 100, 190, 280), rect(5, 113, 190, 280))
    };
    let place = popup_place(&menu, &parent, Surface::shown(&parent, 1.0));
    assert_eq!(place, rect(5, 72, 190, 280));
    let negative = Cell { rel: Point { x: -40, y: -6 }, ..menu.clone() };
    assert_eq!(popup_place(&negative, &parent, Surface::shown(&parent, 1.0)).x, -40, "a popup may stick out");
    let shown_at_half = Surface { w: 443.0, h: 296.5 };
    assert_eq!(popup_place(&menu, &parent, shown_at_half), rect(3, 36, 95, 140));
}

#[test]
fn the_drag_strip_is_the_title_bar_less_the_buttons() {
    let window = cell(rect(0, 0, 1200, 800), rect(0, 0, 1200, 800));
    assert_eq!(drag_strip(&window), Some(rect(0, 0, 1062, 32)));
    let surface = Surface::shown(&window, 1.0);
    assert!(on_drag_strip(&window, surface, 500.0, 10.0));
    assert!(!on_drag_strip(&window, surface, 1100.0, 10.0), "the buttons get their clicks");
    assert!(!on_drag_strip(&window, surface, 500.0, 32.0), "the strip ends at its height");
    assert!(on_drag_strip(&window, Surface { w: 600.0, h: 400.0 }, 500.0, 10.0));
    assert!(!on_drag_strip(&window, Surface { w: 600.0, h: 400.0 }, 540.0, 10.0));
}

#[test]
fn no_strip_where_the_helper_does_not_know_the_title_bar() {
    let mut window = cell(rect(0, 0, 100, 100), rect(0, 0, 100, 100));
    window.caption = Caption { h: 0, buttons_w: 0 };
    assert_eq!(drag_strip(&window), None);
    assert!(!on_drag_strip(&window, Surface::shown(&window, 1.0), 5.0, 5.0));
    window.caption = Caption { h: 32, buttons_w: 200 };
    assert_eq!(drag_strip(&window), None, "the buttons are wider than the window");
    window.caption = Caption { h: 500, buttons_w: 0 };
    assert_eq!(drag_strip(&window), Some(rect(0, 0, 100, 100)), "never taller than the window");
}

#[test]
fn the_nearest_step_is_taken_and_the_smaller_of_two_as_near() {
    let all = &WINDOWS_STEPS[..];
    assert_eq!(nearest_step(1.0, all), Some(100));
    assert_eq!(nearest_step(1.3, all), Some(125));
    assert_eq!(nearest_step(1.375, all), Some(125), "a tie goes to the smaller");
    assert_eq!(nearest_step(1.4, all), Some(150));
    assert_eq!(nearest_step(2.0, &[100, 125, 150, 175]), Some(175), "only what is offered");
    assert_eq!(nearest_step(1.5, &[]), None);
}

#[test]
fn a_cell_is_one_pixel_a_pixel_where_windows_is_at_the_monitors_step() {
    let offered = &WINDOWS_STEPS[..];
    for (scale, percent) in [(1.0, 100), (1.25, 125), (1.5, 150), (1.75, 175), (2.0, 200)] {
        assert_eq!(shown_ratio(scale, percent, offered, Fit::Sharp), scale);
        assert_eq!(shown_ratio(scale, percent, offered, Fit::Size), scale);
    }
    // 130 % is no step: Windows is at 125 %.
    assert_eq!(shown_ratio(1.3, 125, offered, Fit::Sharp), 1.3, "sharp, and 4 % small");
    assert_eq!(shown_ratio(1.3, 125, offered, Fit::Size), 1.25, "its size, scaled by the compositor");
    // A window on another monitor than the one Windows follows.
    assert_eq!(shown_ratio(1.0, 150, offered, Fit::Sharp), 1.5);
    assert_eq!(shown_ratio(2.0, 125, offered, Fit::Sharp), 1.25);
}

#[test]
fn a_logical_length_holds_every_pixel_and_is_under_a_unit_over() {
    for ratio in [1.0, 1.25, 1.3, 1.5, 1.75, 2.0, 1.45, 121.0 / 120.0] {
        for pixels in 1..1500 {
            let length = logical_length(pixels, ratio);
            let device = f64::from(length) * ratio;
            assert!(device + 1e-6 >= f64::from(pixels), "{ratio} {pixels}");
            assert!(device - f64::from(pixels) < ratio + 1e-6, "{ratio} {pixels}");
        }
    }
    assert_eq!(logical_length(1300, 1.3), 1000);
    assert_eq!(logical_length(1301, 1.3), 1001);
}

#[test]
fn a_point_of_a_cell_shown_at_a_fractional_scale_is_the_pixel_under_it() {
    let window = cell(rect(300, 200, 1201, 801), rect(300, 200, 1201, 801));
    let surface = Surface::shown(&window, 1.3);
    // Logical 10 at 1.3 is device 13: the 14th pixel, number 13.
    assert_eq!(guest_point(&window, surface, 10.0, 10.0, SCREEN), Point { x: 313, y: 213 });
    assert_eq!(guest_point(&window, surface, 10.76, 0.0, SCREEN), Point { x: 313, y: 200 });
    assert_eq!(guest_point(&window, surface, 10.77, 0.0, SCREEN), Point { x: 314, y: 200 });
    assert_eq!(
        guest_point(&window, surface, 923.8, 615.9, SCREEN),
        Point { x: 1500, y: 1000 },
        "the last pixel"
    );
}
