//! Device-pixel geometry shared by the Win32 modules. Serializable so recorded taskbar layouts
//! can be stored as JSON (`tests/layouts/`).

use serde::{Deserialize, Serialize};

/// A rectangle in device px, `right`/`bottom` exclusive (as Win32 `RECT`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rect {
    /// Left edge.
    pub left: i32,
    /// Top edge.
    pub top: i32,
    /// Right edge (exclusive).
    pub right: i32,
    /// Bottom edge (exclusive).
    pub bottom: i32,
}

impl Rect {
    /// Width.
    pub const fn width(&self) -> i32 {
        self.right - self.left
    }

    /// Height.
    pub const fn height(&self) -> i32 {
        self.bottom - self.top
    }

    /// The centre, rounded down.
    pub const fn center(&self) -> Point {
        Point {
            x: self.left + self.width() / 2,
            y: self.top + self.height() / 2,
        }
    }

    /// `p` lies inside.
    pub const fn contains(&self, p: Point) -> bool {
        p.x >= self.left && p.x < self.right && p.y >= self.top && p.y < self.bottom
    }
}

/// A point in device px.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Point {
    /// x.
    pub x: i32,
    /// y.
    pub y: i32,
}

/// A size in device px.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Size {
    /// Width.
    pub width: i32,
    /// Height.
    pub height: i32,
}

/// A rectangle in fractional device px (Direct2D coordinates).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RectF {
    /// Left.
    pub x: f32,
    /// Top.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

/// A point in fractional device px.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PointF {
    /// x.
    pub x: f32,
    /// y.
    pub y: f32,
}
