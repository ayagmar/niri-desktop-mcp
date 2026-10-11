//! What the accessibility tree says, and where that is in niri's layout (pure). A
//! toolkit reports an element's extents relative to its window (AT-SPI's WINDOW
//! coordinates); on Wayland its screen coordinates are meaningless. niri knows where the
//! window is, so an element's layout box is the window's place in the layout plus those
//! extents. That holds only when the toolkit's window origin is niri's window geometry,
//! which the frame guard checks: the frame's own WINDOW size must equal niri's
//! `window_size`. GTK 3 measures from its surface's shadow and Qt from inside its own
//! decorations, and both fail the guard with client-side decorations.

use std::hash::{DefaultHasher, Hash, Hasher};

use niri_ipc::{LogicalOutput, WindowLayout};
use serde::Serialize;

use crate::coords::LayoutPt;

/// AT-SPI's roles, by number, in at-spi2-core 2.60's `AtspiRole` order, named in snake case.
const ROLES: [&str; 131] = [
    "invalid",
    "accelerator_label",
    "alert",
    "animation",
    "arrow",
    "calendar",
    "canvas",
    "check_box",
    "check_menu_item",
    "color_chooser",
    "column_header",
    "combo_box",
    "date_editor",
    "desktop_icon",
    "desktop_frame",
    "dial",
    "dialog",
    "directory_pane",
    "drawing_area",
    "file_chooser",
    "filler",
    "focus_traversable",
    "font_chooser",
    "frame",
    "glass_pane",
    "html_container",
    "icon",
    "image",
    "internal_frame",
    "label",
    "layered_pane",
    "list",
    "list_item",
    "menu",
    "menu_bar",
    "menu_item",
    "option_pane",
    "page_tab",
    "page_tab_list",
    "panel",
    "password_text",
    "popup_menu",
    "progress_bar",
    "button",
    "radio_button",
    "radio_menu_item",
    "root_pane",
    "row_header",
    "scroll_bar",
    "scroll_pane",
    "separator",
    "slider",
    "spin_button",
    "split_pane",
    "status_bar",
    "table",
    "table_cell",
    "table_column_header",
    "table_row_header",
    "tearoff_menu_item",
    "terminal",
    "text",
    "toggle_button",
    "tool_bar",
    "tool_tip",
    "tree",
    "tree_table",
    "unknown",
    "viewport",
    "window",
    "extended",
    "header",
    "footer",
    "paragraph",
    "ruler",
    "application",
    "autocomplete",
    "editbar",
    "embedded",
    "entry",
    "chart",
    "caption",
    "document_frame",
    "heading",
    "page",
    "section",
    "redundant_object",
    "form",
    "link",
    "input_method_window",
    "table_row",
    "tree_item",
    "document_spreadsheet",
    "document_presentation",
    "document_text",
    "document_web",
    "document_email",
    "comment",
    "list_box",
    "grouping",
    "image_map",
    "notification",
    "info_bar",
    "level_bar",
    "title_bar",
    "block_quote",
    "audio",
    "video",
    "definition",
    "article",
    "landmark",
    "log",
    "marquee",
    "math",
    "rating",
    "timer",
    "static",
    "math_fraction",
    "math_root",
    "subscript",
    "superscript",
    "description_list",
    "description_term",
    "description_value",
    "footnote",
    "content_deletion",
    "content_insertion",
    "mark",
    "suggestion",
    "push_button_menu",
    "switch",
];
/// AT-SPI's states, by bit, in `AtspiStateType` order, named in snake case.
const STATES: [&str; 44] = [
    "invalid",
    "active",
    "armed",
    "busy",
    "checked",
    "collapsed",
    "defunct",
    "editable",
    "enabled",
    "expandable",
    "expanded",
    "focusable",
    "focused",
    "has_tooltip",
    "horizontal",
    "iconified",
    "modal",
    "multi_line",
    "multiselectable",
    "opaque",
    "pressed",
    "resizable",
    "selectable",
    "selected",
    "sensitive",
    "showing",
    "single_line",
    "stale",
    "transient",
    "vertical",
    "visible",
    "manages_descendants",
    "indeterminate",
    "required",
    "truncated",
    "animated",
    "invalid_entry",
    "supports_autocompletion",
    "selectable_text",
    "is_default",
    "visited",
    "checkable",
    "has_popup",
    "read_only",
];

/// A role's name, or `unknown` for a number newer than this table.
pub(crate) fn role_name(role: u32) -> &'static str {
    usize::try_from(role)
        .ok()
        .and_then(|index| ROLES.get(index))
        .copied()
        .unwrap_or("unknown")
}

/// Whether `name` is a role this table knows, for checking a `role` argument.
pub(crate) fn is_role(name: &str) -> bool {
    ROLES.contains(&name)
}

/// An AT-SPI state, by its bit in the state set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum State {
    Editable = 7,
    Showing = 25,
    Visible = 30,
}

/// A state set as `Accessible.GetState` returns it: two 32-bit words, low bits first.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct States(u64);

impl States {
    pub(crate) fn from_words(words: &[u32]) -> Self {
        let word = |index: usize| u64::from(words.get(index).copied().unwrap_or(0));
        Self(word(0) | (word(1) << 32))
    }

    pub(crate) const fn has(self, state: State) -> bool {
        self.0 & (1 << state as u64) != 0
    }

    /// The names of the states set, in bit order. Bits past the table are left out.
    pub(crate) fn names(self) -> Vec<&'static str> {
        STATES
            .iter()
            .enumerate()
            .filter(|(bit, _)| self.0 & (1 << bit) != 0)
            .map(|(_, name)| *name)
            .collect()
    }

    /// The names of the states set now but not `before`, then of those cleared since.
    pub(crate) fn changes(self, before: Self) -> (Vec<&'static str>, Vec<&'static str>) {
        (
            Self(self.0 & !before.0).names(),
            Self(before.0 & !self.0).names(),
        )
    }
}

/// A rectangle as `Component.GetExtents` returns it, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct Extents {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: i32,
    pub(crate) height: i32,
}

/// A rectangle in niri's layout space, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub(crate) struct LayoutBox {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
}

impl LayoutBox {
    pub(crate) const fn centre(self) -> LayoutPt {
        LayoutPt {
            x: self.width.mul_add(0.5, self.x),
            y: self.height.mul_add(0.5, self.y),
        }
    }
}

/// Why an element has no layout box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Unmappable {
    /// The window's accessible frame isn't the size niri gives the window, so the
    /// toolkit's window coordinates start elsewhere.
    FrameSizeMismatch,
    /// The element isn't showing and visible, or niri has no place for the window in its
    /// workspace's view, as on a workspace that isn't shown.
    NotShowing,
    /// The element has no area.
    Empty,
    /// The element's centre is outside the screenshot the pointer aims through.
    OutsideScreenshot,
}

/// Where the window's geometry starts in the layout: the output's origin, plus the tile's
/// place in the workspace's view, plus the window's offset in its tile. None while niri
/// gives the tile no place, as on a workspace that isn't shown.
pub(crate) fn window_origin(output: &LogicalOutput, layout: &WindowLayout) -> Option<LayoutPt> {
    let (tile_x, tile_y) = layout.tile_pos_in_workspace_view?;
    let (offset_x, offset_y) = layout.window_offset_in_tile;
    Some(LayoutPt {
        x: f64::from(output.x) + tile_x + offset_x,
        y: f64::from(output.y) + tile_y + offset_y,
    })
}

/// The frame guard: the frame's WINDOW extents are niri's window size, give or take a
/// pixel of rounding.
pub(crate) const fn frame_fits(frame: Extents, window_size: (i32, i32)) -> bool {
    (frame.width - window_size.0).abs() <= 1 && (frame.height - window_size.1).abs() <= 1
}

/// What decides whether an element can be aimed at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Placement {
    pub(crate) states: States,
    pub(crate) extents: Extents,
    pub(crate) frame_fits: bool,
    pub(crate) origin: Option<LayoutPt>,
}

/// The element's box in the layout, or why it has none.
pub(crate) fn place(placement: Placement) -> Result<LayoutBox, Unmappable> {
    let Placement {
        states,
        extents,
        frame_fits,
        origin,
    } = placement;
    if !(states.has(State::Showing) && states.has(State::Visible)) {
        return Err(Unmappable::NotShowing);
    }
    if extents.width <= 0 || extents.height <= 0 {
        return Err(Unmappable::Empty);
    }
    if !frame_fits {
        return Err(Unmappable::FrameSizeMismatch);
    }
    let origin = origin.ok_or(Unmappable::NotShowing)?;
    Ok(LayoutBox {
        x: origin.x + f64::from(extents.x),
        y: origin.y + f64::from(extents.y),
        width: f64::from(extents.width),
        height: f64::from(extents.height),
    })
}

/// One of an application's top-level frames.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Frame<'a> {
    pub(crate) extents: Extents,
    /// The frame's accessible name, which toolkits set to the window title.
    pub(crate) name: &'a str,
}

/// The frame that is niri's window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Picked {
    /// The frame at this index passes the frame guard.
    Fits(usize),
    /// The application's only frame, or the only one with the window's title, fails it.
    Mismatch(usize),
}

/// Why no frame is niri's window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoFrame {
    /// The application has no frames.
    None,
    /// Several frames fit, and the title doesn't tell them apart.
    Ambiguous(usize),
}

/// Picks the frame that is niri's window among an application's frames: the one that
/// passes the frame guard, or failing that the only frame. When several qualify, only the
/// one whose name is the window's title is taken.
pub(crate) fn pick_frame(
    frames: &[Frame<'_>],
    window_size: (i32, i32),
    title: Option<&str>,
) -> Result<Picked, NoFrame> {
    let fitting: Vec<usize> = (0..frames.len())
        .filter(|&index| {
            frames
                .get(index)
                .is_some_and(|frame| frame_fits(frame.extents, window_size))
        })
        .collect();
    if !fitting.is_empty() {
        return only(frames, &fitting, title).map(Picked::Fits);
    }
    let all: Vec<usize> = (0..frames.len()).collect();
    only(frames, &all, title).map(Picked::Mismatch)
}

/// The one candidate, or the one whose name is `title`.
fn only(frames: &[Frame<'_>], candidates: &[usize], title: Option<&str>) -> Result<usize, NoFrame> {
    match candidates {
        [] => Err(NoFrame::None),
        [one] => Ok(*one),
        several => {
            let titled: Vec<usize> = several
                .iter()
                .copied()
                .filter(|&index| {
                    frames
                        .get(index)
                        .is_some_and(|frame| title.is_some_and(|title| frame.name == title))
                })
                .collect();
            match titled.as_slice() {
                [one] => Ok(*one),
                _ => Err(NoFrame::Ambiguous(several.len())),
            }
        }
    }
}

/// The `elements` filters: an exact role name, and text the name contains, ignoring case.
#[derive(Debug, Clone, Default)]
pub(crate) struct Filter {
    pub(crate) role: Option<String>,
    /// Lowercased.
    name_contains: Option<String>,
}

impl Filter {
    pub(crate) fn new(role: Option<String>, name_contains: Option<&str>) -> Self {
        Self {
            role,
            name_contains: name_contains.map(str::to_lowercase),
        }
    }

    pub(crate) fn matches(&self, role: &str, name: &str) -> bool {
        self.role.as_deref().is_none_or(|wanted| wanted == role)
            && self
                .name_contains
                .as_deref()
                .is_none_or(|part| name.to_lowercase().contains(part))
    }
}

/// An element as a ref keeps it, to check again before aiming at it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Kept {
    pub(crate) role: u32,
    pub(crate) window: u64,
    pub(crate) pid: i32,
    /// The action names it listed, which `activate_element` chooses from.
    pub(crate) actions: Vec<String>,
    /// Where it was in its window's tree, to tell it apart from another object that took
    /// its path, as a virtualized list reuses its rows: each object from the frame's child
    /// down to the element, the element last. Never logged or returned.
    pub(crate) lineage: Vec<Link>,
}

/// One object on the way from a window's frame down to an element: its path, its index
/// among its parent's children, and its accessible name's hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Link {
    pub(crate) path: String,
    pub(crate) index: i32,
    pub(crate) name: NameHash,
}

/// A hash of an accessible name, to tell whether the name changed without keeping it.
/// Kept in memory only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NameHash(u64);

impl NameHash {
    pub(crate) fn of(name: &str) -> Self {
        let mut hasher = DefaultHasher::new();
        name.hash(&mut hasher);
        Self(hasher.finish())
    }
}

/// The element and its window as they are now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Fresh {
    pub(crate) role: u32,
    pub(crate) placement: Placement,
}

/// Why a ref can't be aimed at any more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refused {
    /// The element is gone or is now something else.
    Stale(String),
    Unmappable(Unmappable),
}

/// Checks a kept element against how it is now: the same role, then a layout box.
pub(crate) fn recheck(kept: &Kept, fresh: Fresh) -> Result<LayoutBox, Refused> {
    if fresh.role != kept.role {
        return Err(Refused::Stale(format!(
            "the element was a {} and is now a {}",
            role_name(kept.role),
            role_name(fresh.role)
        )));
    }
    place(fresh.placement).map_err(Refused::Unmappable)
}

#[cfg(test)]
mod tests {
    use niri_ipc::Transform;

    use super::*;

    const SHOWN: States = States((1 << 25) | (1 << 30));

    fn output(x: i32, y: i32, scale: f64) -> LogicalOutput {
        LogicalOutput {
            x,
            y,
            width: 960,
            height: 720,
            scale,
            transform: Transform::Flipped180,
        }
    }

    fn layout(tile: Option<(f64, f64)>, offset: (f64, f64)) -> WindowLayout {
        WindowLayout {
            pos_in_scrolling_layout: None,
            tile_size: (400.0, 300.0),
            window_size: (400, 300),
            tile_pos_in_workspace_view: tile,
            window_offset_in_tile: offset,
        }
    }

    const fn extents(x: i32, y: i32, width: i32, height: i32) -> Extents {
        Extents {
            x,
            y,
            width,
            height,
        }
    }

    fn placement(extents: Extents, origin: Option<LayoutPt>) -> Placement {
        Placement {
            states: SHOWN,
            extents,
            frame_fits: true,
            origin,
        }
    }

    #[test]
    fn a_box_is_the_windows_place_in_the_layout_plus_its_window_extents() {
        // GTK 4 with CSD at scale 1: the Primary button, its tile at (20, 54).
        let origin = window_origin(&output(0, 0, 1.0), &layout(Some((20.0, 54.0)), (0.0, 0.0)));
        let primary = place(placement(extents(40, 109, 320, 34), origin)).unwrap();
        assert_eq!(
            primary,
            LayoutBox {
                x: 60.0,
                y: 163.0,
                width: 320.0,
                height: 34.0
            }
        );
        assert_eq!(primary.centre(), LayoutPt { x: 220.0, y: 180.0 });
    }

    #[test]
    fn fractional_tile_positions_offsets_and_output_origins_carry_through() {
        // At scale 1.5 niri places tiles at fractional logical positions.
        let origin = window_origin(
            &output(-1280, 100, 1.5),
            &layout(Some((57.333_333, 65.333_333)), (1.5, 2.25)),
        )
        .unwrap();
        let found = place(placement(extents(40, 70, 320, 34), Some(origin))).unwrap();
        assert!((found.x - (-1280.0 + 57.333_333 + 1.5 + 40.0)).abs() < 1e-9);
        assert!((found.y - (100.0 + 65.333_333 + 2.25 + 70.0)).abs() < 1e-9);
        assert!((found.centre().x - (found.x + 160.0)).abs() < 1e-9);
    }

    #[test]
    fn the_frame_guard_allows_a_pixel_of_rounding_and_nothing_more() {
        let fits = |width, height| frame_fits(extents(0, 0, width, height), (400, 300));
        assert!(fits(400, 300));
        assert!(fits(401, 299));
        assert!(fits(399, 301));
        // GTK 3 measures from its shadow, Qt from inside its decorations.
        assert!(!fits(452, 352));
        assert!(!fits(394, 267));
        assert!(!fits(402, 300));
    }

    #[test]
    fn hidden_empty_and_unplaced_elements_have_no_box() {
        let origin = Some(LayoutPt { x: 0.0, y: 0.0 });
        let hidden = Placement {
            states: States(1 << 30),
            ..placement(extents(1, 1, 10, 10), origin)
        };
        assert_eq!(place(hidden), Err(Unmappable::NotShowing));
        // Qt lists widgets on hidden tabs with no size.
        assert_eq!(
            place(placement(extents(0, 0, 0, 0), origin)),
            Err(Unmappable::Empty)
        );
        let mismatch = Placement {
            frame_fits: false,
            ..placement(extents(66, 131, 320, 34), origin)
        };
        assert_eq!(place(mismatch), Err(Unmappable::FrameSizeMismatch));
        // A window on a workspace that isn't shown has no place in the view.
        assert_eq!(
            window_origin(&output(0, 0, 1.0), &layout(None, (0.0, 0.0))),
            None
        );
        assert_eq!(
            place(placement(extents(1, 1, 10, 10), None)),
            Err(Unmappable::NotShowing)
        );
    }

    #[test]
    fn picks_the_frame_that_is_the_window() {
        let frame = |width, height, name| Frame {
            extents: extents(0, 0, width, height),
            name,
        };
        let size = (400, 300);
        assert_eq!(
            pick_frame(
                &[frame(200, 100, "about"), frame(400, 300, "main")],
                size,
                None
            ),
            Ok(Picked::Fits(1))
        );
        // One frame that fails the guard is still the window, but unmappable.
        assert_eq!(
            pick_frame(&[frame(452, 352, "gtk3")], size, Some("gtk3")),
            Ok(Picked::Mismatch(0))
        );
        let twins = [frame(400, 300, "one"), frame(400, 300, "two")];
        assert_eq!(pick_frame(&twins, size, Some("two")), Ok(Picked::Fits(1)));
        assert_eq!(
            pick_frame(&twins, size, Some("three")),
            Err(NoFrame::Ambiguous(2))
        );
        assert_eq!(pick_frame(&twins, size, None), Err(NoFrame::Ambiguous(2)));
        let unfit = [frame(10, 10, "a"), frame(20, 20, "b")];
        assert_eq!(pick_frame(&unfit, size, Some("b")), Ok(Picked::Mismatch(1)));
        assert_eq!(pick_frame(&[], size, None), Err(NoFrame::None));
    }

    #[test]
    fn filters_by_exact_role_and_name_text_in_any_case() {
        let all = Filter::default();
        assert!(all.matches("label", ""));
        let buttons = Filter::new(Some("button".to_owned()), Some("PRIMARY"));
        assert!(buttons.matches("button", "Primary: 3"));
        assert!(!buttons.matches("button", "Second: 0"));
        assert!(!buttons.matches("toggle_button", "Primary"));
        assert!(is_role("check_box"));
        assert!(!is_role("push button"));
    }

    #[test]
    fn roles_and_states_are_named_from_the_at_spi_tables() {
        assert_eq!(role_name(23), "frame");
        assert_eq!(role_name(43), "button");
        assert_eq!(role_name(61), "text");
        assert_eq!(role_name(130), "switch");
        assert_eq!(role_name(131), "unknown");
        let states = States::from_words(&[(1 << 8) | (1 << 25) | (1 << 30), 1 << 11]);
        assert!(states.has(State::Showing) && states.has(State::Visible));
        assert!(!States::from_words(&[1 << 30]).has(State::Showing));
        assert_eq!(
            states.names(),
            ["enabled", "showing", "visible", "read_only"]
        );
        assert_eq!(States::from_words(&[]), States::default());
        let pressed = States::from_words(&[(1 << 8) | (1 << 20) | (1 << 30)]);
        assert_eq!(
            pressed.changes(states),
            (vec!["pressed"], vec!["showing", "read_only"])
        );
        assert_eq!(states.changes(states), (vec![], vec![]));
    }

    #[test]
    fn a_kept_element_is_stale_once_it_changes_role_and_unmappable_once_hidden() {
        let kept = Kept {
            role: 43,
            window: 3,
            pid: 4711,
            actions: Vec::new(),
            lineage: Vec::new(),
        };
        let shown = placement(
            extents(40, 70, 320, 34),
            Some(LayoutPt { x: 20.0, y: 20.0 }),
        );
        let fresh = |role, placement| Fresh { role, placement };
        assert_eq!(
            recheck(&kept, fresh(43, shown)).map(LayoutBox::centre),
            Ok(LayoutPt { x: 220.0, y: 107.0 })
        );
        assert!(matches!(
            recheck(&kept, fresh(29, shown)),
            Err(Refused::Stale(detail)) if detail.contains("button") && detail.contains("label")
        ));
        let hidden = Placement {
            states: States::default(),
            ..shown
        };
        assert_eq!(
            recheck(&kept, fresh(43, hidden)),
            Err(Refused::Unmappable(Unmappable::NotShowing))
        );
    }
}
