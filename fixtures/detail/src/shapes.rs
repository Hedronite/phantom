//! Data shapes.

/// Meters along a track. Never negative.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Meters(pub f64);

/// Marks the end of a run
pub struct Stop;

/**
 * Compass headings, clockwise.
 *
 * Four of them.
 */
#[derive(Debug, Clone, Copy)]
pub enum Heading {
    North,
    East,
    South,
    West,
}

/// A square grid of cells.
#[derive(Debug)]
#[non_exhaustive]
pub struct Grid<T: Copy, const N: usize>
where
    T: Default,
{
    cells: [T; N],
}

/** Things with an area. */
pub trait Area: Sized {
    /// The area in square units.
    fn area(&self) -> f64;

    /// Twice the area.
    fn doubled(&self) -> f64 {
        self.area() * 2.0
    }
}

impl<T: Copy + Default, const N: usize> Grid<T, N> {
    /// Builds a grid with every cell set to one value.
    pub fn filled(value: T) -> Self {
        Self { cells: [value; N] }
    }

    fn len(&self) -> usize {
        N
    }
}

impl Area for Meters {
    /// Treats the length as a side.
    #[inline]
    fn area(&self) -> f64 {
        self.0 * self.0
    }
}
