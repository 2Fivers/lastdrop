//! `flyout_pos::flyout_rect` (F§1.3) and `hover_card_rect` (R§8.1) on recorded-style layouts:
//! 1080p and 1440p monitors at 100/125/150/200 %, bottom and top taskbars, anchors in the middle
//! and at both edges, a secondary monitor left of the primary (negative coordinates), and point
//! anchors (tray icon in the overflow).

use lastdrop_win::flyout_pos::{Anchor, MonitorInfo, flyout_rect, hover_card_rect};
use lastdrop_win::geom::{Point, Rect, Size};
use lastdrop_win::placement::TaskbarEdge;

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
    Rect {
        left,
        top,
        right,
        bottom,
    }
}

/// A monitor of `w × h` at `x` with a `bar`-high taskbar on `edge`.
fn monitor(x: i32, w: i32, h: i32, bar: i32, edge: TaskbarEdge, dpi: u32) -> (MonitorInfo, Rect) {
    let screen = rect(x, 0, x + w, h);
    let (work, taskbar) = match edge {
        TaskbarEdge::Bottom => (rect(x, 0, x + w, h - bar), rect(x, h - bar, x + w, h)),
        TaskbarEdge::Top => (rect(x, bar, x + w, h), rect(x, 0, x + w, bar)),
        TaskbarEdge::Left => (rect(x + bar, 0, x + w, h), rect(x, 0, x + bar, h)),
        TaskbarEdge::Right => (rect(x, 0, x + w - bar, h), rect(x + w - bar, 0, x + w, h)),
    };
    (
        MonitorInfo {
            monitor: screen,
            work,
            dpi,
        },
        taskbar,
    )
}

/// `round(x × s)` half up (R§0).
fn s(x: i32, dpi: u32) -> i32 {
    (2 * x * dpi as i32 + 96) / 192
}

const SCALES: [u32; 4] = [96, 120, 144, 192];

#[test]
fn flyout_sits_12_above_a_bottom_taskbar_and_12_from_the_right_edge() {
    for dpi in SCALES {
        let bar = s(48, dpi);
        let (m, _) = monitor(0, 1920, 1080, bar, TaskbarEdge::Bottom, dpi);
        // F§1.2: height from content, at most the work area's height − 24 (200 % on 1080p).
        let size = Size {
            width: s(360, dpi),
            height: s(478, dpi).min(m.work.height() - s(24, dpi)),
        };
        // Tray icon near the right end, like the clock's neighbour.
        let icon = rect(
            1920 - s(140, dpi),
            1080 - bar + s(8, dpi),
            1920 - s(108, dpi),
            1080 - s(8, dpi),
        );
        let r = flyout_rect(Anchor::Rect(icon), size, &m, TaskbarEdge::Bottom);
        assert_eq!(
            r.right,
            1920 - s(12, dpi),
            "dpi {dpi}: clamped to the right margin"
        );
        assert_eq!(r.bottom, m.work.bottom - s(12, dpi), "dpi {dpi}");
        assert_eq!((r.width(), r.height()), (size.width, size.height));
    }
}

#[test]
fn flyout_is_centred_on_an_anchor_with_room() {
    let (m, _) = monitor(0, 2560, 1440, 48, TaskbarEdge::Bottom, 96);
    let size = Size {
        width: 360,
        height: 478,
    };
    // A readout slot in the middle of the taskbar.
    let slot = rect(1000, 1392, 1160, 1432);
    let r = flyout_rect(Anchor::Rect(slot), size, &m, TaskbarEdge::Bottom);
    assert_eq!(r.left, 1080 - 180);
    assert_eq!(r.right, 1080 + 180);
    assert_eq!(r.bottom, 1392 - 12);
    // Odd anchor width: the centre is 1080.5, rounded down.
    let r = flyout_rect(
        Anchor::Rect(rect(1000, 1392, 1161, 1432)),
        size,
        &m,
        TaskbarEdge::Bottom,
    );
    assert_eq!(r.left, 900);
}

#[test]
fn flyout_clamps_at_the_left_edge_and_follows_a_top_taskbar() {
    for dpi in SCALES {
        let bar = s(48, dpi);
        let (m, _) = monitor(0, 1920, 1080, bar, TaskbarEdge::Top, dpi);
        let size = Size {
            width: s(360, dpi),
            height: s(300, dpi),
        };
        let r = flyout_rect(
            Anchor::Point(Point { x: 3, y: 10 }),
            size,
            &m,
            TaskbarEdge::Top,
        );
        assert_eq!(r.left, s(12, dpi));
        assert_eq!(r.top, bar + s(12, dpi));
    }
}

#[test]
fn flyout_on_a_monitor_left_of_the_primary() {
    let (m, _) = monitor(-1920, 1920, 1080, 48, TaskbarEdge::Bottom, 96);
    let size = Size {
        width: 360,
        height: 400,
    };
    let r = flyout_rect(
        Anchor::Point(Point { x: -15, y: 1060 }),
        size,
        &m,
        TaskbarEdge::Bottom,
    );
    assert_eq!(r.right, -12);
    assert_eq!(r.left, -372);
    let r = flyout_rect(
        Anchor::Point(Point { x: -1000, y: 1060 }),
        size,
        &m,
        TaskbarEdge::Bottom,
    );
    assert_eq!(r.left, -1000 - 180);
}

#[test]
fn a_flyout_taller_than_the_work_area_starts_at_the_top_margin() {
    let (m, _) = monitor(0, 1280, 600, 48, TaskbarEdge::Bottom, 96);
    let size = Size {
        width: 360,
        height: 700,
    };
    let r = flyout_rect(
        Anchor::Point(Point { x: 640, y: 580 }),
        size,
        &m,
        TaskbarEdge::Bottom,
    );
    assert_eq!(r.top, 12);
}

#[test]
fn hover_card_sits_8_above_the_taskbar_centred_on_the_readout() {
    for dpi in SCALES {
        let bar = s(48, dpi);
        let (m, taskbar) = monitor(0, 1920, 1080, bar, TaskbarEdge::Bottom, dpi);
        let size = Size {
            width: s(320, dpi),
            height: s(180, dpi),
        };
        let slot = rect(
            1200,
            taskbar.top + s(4, dpi),
            1200 + s(176, dpi),
            taskbar.bottom - s(4, dpi),
        );
        let r = hover_card_rect(slot, size, &m, taskbar, TaskbarEdge::Bottom);
        assert_eq!(r.bottom, taskbar.top - s(8, dpi), "dpi {dpi}");
        assert_eq!(
            r.left,
            (slot.left + slot.right - size.width).div_euclid(2),
            "dpi {dpi}"
        );
        assert_eq!(r.width(), size.width);
    }
}

#[test]
fn hover_card_clamps_12_inside_the_work_area() {
    let (m, taskbar) = monitor(0, 1920, 1080, 48, TaskbarEdge::Bottom, 96);
    let size = Size {
        width: 320,
        height: 160,
    };
    let icon = rect(1890, 1040, 1914, 1064);
    let r = hover_card_rect(icon, size, &m, taskbar, TaskbarEdge::Bottom);
    assert_eq!(r.right, 1920 - 12);
    let icon = rect(2, 1040, 26, 1064);
    let r = hover_card_rect(icon, size, &m, taskbar, TaskbarEdge::Bottom);
    assert_eq!(r.left, 12);
}

#[test]
fn a_hover_card_taller_than_the_space_starts_at_the_top_margin() {
    let (m, taskbar) = monitor(0, 1280, 400, 48, TaskbarEdge::Bottom, 96);
    let size = Size {
        width: 320,
        height: 380,
    };
    let r = hover_card_rect(
        rect(600, 356, 700, 396),
        size,
        &m,
        taskbar,
        TaskbarEdge::Bottom,
    );
    assert_eq!(r.top, 12);
}

#[test]
fn hover_card_opens_below_a_top_taskbar() {
    let (m, taskbar) = monitor(0, 1920, 1080, 60, TaskbarEdge::Top, 120);
    let size = Size {
        width: 400,
        height: 200,
    };
    let slot = rect(900, 10, 1100, 50);
    let r = hover_card_rect(slot, size, &m, taskbar, TaskbarEdge::Top);
    assert_eq!(r.top, 60 + 10);
    assert_eq!(r.left, 800);
}

#[test]
fn hover_card_beside_a_vertical_taskbar() {
    let (m, taskbar) = monitor(0, 1920, 1080, 62, TaskbarEdge::Left, 96);
    let size = Size {
        width: 320,
        height: 160,
    };
    let icon = rect(19, 1000, 43, 1024);
    let r = hover_card_rect(icon, size, &m, taskbar, TaskbarEdge::Left);
    assert_eq!(r.left, 62 + 8, "8 beside the taskbar");
    assert_eq!(
        r.bottom,
        1080 - 12,
        "clamped 12 inside the work area along the taskbar"
    );

    let (m, taskbar) = monitor(0, 1920, 1080, 62, TaskbarEdge::Right, 96);
    let icon = rect(1877, 500, 1901, 524);
    let r = hover_card_rect(icon, size, &m, taskbar, TaskbarEdge::Right);
    assert_eq!(r.right, 1858 - 8);
    assert_eq!(r.top, 512 - 80);
}

#[test]
fn popups_never_cover_a_floating_readout() {
    // No room on the taskbar: the readout floats 4 px above it (placement's floating slot).
    for dpi in SCALES {
        let bar = s(48, dpi);
        let (m, taskbar) = monitor(0, 1920, 1080, bar, TaskbarEdge::Bottom, dpi);
        let slot_top = taskbar.top - s(4, dpi) - s(40, dpi);
        let slot = rect(1500, slot_top, 1500 + s(176, dpi), taskbar.top - s(4, dpi));
        let card = Size {
            width: s(320, dpi),
            height: s(180, dpi),
        };
        let r = hover_card_rect(slot, card, &m, taskbar, TaskbarEdge::Bottom);
        assert_eq!(
            r.bottom,
            slot.top - s(8, dpi),
            "dpi {dpi}: card above the readout"
        );
        let flyout = Size {
            width: s(360, dpi),
            height: s(300, dpi),
        };
        let r = flyout_rect(Anchor::Rect(slot), flyout, &m, TaskbarEdge::Bottom);
        assert_eq!(
            r.bottom,
            slot.top - s(12, dpi),
            "dpi {dpi}: flyout above the readout"
        );
    }
    // Top taskbar: the readout floats below it, the popups below the readout.
    let (m, taskbar) = monitor(0, 1920, 1080, 48, TaskbarEdge::Top, 96);
    let slot = rect(1500, 52, 1676, 92);
    let size = Size {
        width: 320,
        height: 180,
    };
    assert_eq!(
        hover_card_rect(slot, size, &m, taskbar, TaskbarEdge::Top).top,
        92 + 8
    );
    assert_eq!(
        flyout_rect(Anchor::Rect(slot), size, &m, TaskbarEdge::Top).top,
        92 + 12
    );
    // A docked readout and a cursor anchor keep the F§1.3 rule.
    let (m, taskbar) = monitor(0, 1920, 1080, 48, TaskbarEdge::Bottom, 96);
    let docked = rect(1500, 1036, 1676, 1076);
    assert_eq!(
        hover_card_rect(docked, size, &m, taskbar, TaskbarEdge::Bottom).bottom,
        1032 - 8
    );
    let cursor = Anchor::Point(Point { x: 1600, y: 900 });
    assert_eq!(
        flyout_rect(cursor, size, &m, TaskbarEdge::Bottom).bottom,
        1032 - 12
    );
}
