//! Geometric primitives shared by layout and paint.

/// Four values in CSS side order: top, right, bottom, left.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Edges<T> {
    /// Top side.
    pub top: T,
    /// Right side.
    pub right: T,
    /// Bottom side.
    pub bottom: T,
    /// Left side.
    pub left: T,
}

impl<T> Edges<T> {
    /// All four sides from one constructor call.
    pub(crate) const fn new(top: T, right: T, bottom: T, left: T) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
        }
    }

    /// Applies `f` to every side.
    pub(crate) fn map<U>(self, mut f: impl FnMut(T) -> U) -> Edges<U> {
        Edges {
            top: f(self.top),
            right: f(self.right),
            bottom: f(self.bottom),
            left: f(self.left),
        }
    }
}

impl Edges<f32> {
    /// Sum of the left and right sides.
    pub(crate) fn horizontal(self) -> f32 {
        self.left + self.right
    }

    /// Sum of the top and bottom sides.
    pub(crate) fn vertical(self) -> f32 {
        self.top + self.bottom
    }
}

/// An axis-aligned rectangle in device pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width; non-positive values are empty.
    pub width: f32,
    /// Height; non-positive values are empty.
    pub height: f32,
}

impl Rect {
    /// A rectangle from an origin and a size.
    pub(crate) const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The right edge.
    pub(crate) fn right(self) -> f32 {
        self.x + self.width
    }

    /// The bottom edge.
    pub(crate) fn bottom(self) -> f32 {
        self.y + self.height
    }

    /// The intersection with `other`, possibly empty.
    pub(crate) fn intersect(self, other: Self) -> Self {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        Self::new(x, y, (right - x).max(0.0), (bottom - y).max(0.0))
    }

    /// Whether the rectangle covers no area.
    pub(crate) fn is_empty(self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }
}
