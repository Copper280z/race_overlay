//! Screen-resolution reduction of plot lines.
//!
//! Camera IMUs record at 1 kHz, so one channel of a three-minute video is
//! ~190 000 samples. A plot only has a few thousand pixel columns, and egui
//! tessellates every point it is given on every repaint. The prepared series
//! stay complete; only what is handed to the plot is reduced, and it is
//! reduced per view so zooming in reveals the underlying samples again.

use std::borrow::Cow;

/// Points kept per pixel column. Four (first, minimum, maximum, last) is
/// enough to keep spikes, edges, and the joins between buckets.
const POINTS_PER_PIXEL: usize = 4;

/// The part of `points` worth drawing for a plot `pixels` wide that currently
/// shows `view` (x range). Lines short enough to draw as they are are returned
/// untouched; longer ones keep each bucket's extremes so peaks stay visible.
///
/// Without a `view` (the first frame, before a plot has reported its bounds)
/// the whole line is reduced. When the x values are not in order (position
/// along a course can move backwards) the line cannot be cut to a range, so
/// it is reduced as a whole instead.
pub(super) fn view_points(
    points: &[[f64; 2]],
    view: Option<[f64; 2]>,
    pixels: usize,
) -> Cow<'_, [[f64; 2]]> {
    let budget = pixels.max(64) * POINTS_PER_PIXEL;
    if points.len() <= budget {
        return Cow::Borrowed(points);
    }
    let ordered = points.windows(2).all(|pair| pair[0][0] <= pair[1][0]);
    let slice = match view {
        Some([low, high]) if ordered && low.is_finite() && high.is_finite() && low < high => {
            // Keep a half view of margin on each side so a pan between two
            // frames never shows an undrawn edge, plus one neighbour so the
            // line leaves the view instead of stopping inside it.
            let margin = (high - low) * 0.5;
            let start = points.partition_point(|p| p[0] < low - margin);
            let end = points.partition_point(|p| p[0] <= high + margin);
            &points[start.saturating_sub(1)..(end + 1).min(points.len())]
        }
        _ => points,
    };
    if slice.len() <= budget {
        return Cow::Borrowed(slice);
    }
    let buckets = budget / POINTS_PER_PIXEL;
    let per_bucket = slice.len().div_ceil(buckets);
    let mut reduced = Vec::with_capacity(budget + POINTS_PER_PIXEL);
    for chunk in slice.chunks(per_bucket) {
        let (mut low, mut high) = (0, 0);
        for (index, point) in chunk.iter().enumerate() {
            if point[1] < chunk[low][1] {
                low = index;
            }
            if point[1] > chunk[high][1] {
                high = index;
            }
        }
        let mut keep = [0, low.min(high), low.max(high), chunk.len() - 1];
        keep.sort_unstable();
        let mut previous = None;
        for index in keep {
            if previous != Some(index) {
                reduced.push(chunk[index]);
                previous = Some(index);
            }
        }
    }
    Cow::Owned(reduced)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(count: usize) -> Vec<[f64; 2]> {
        (0..count).map(|i| [i as f64, (i % 50) as f64]).collect()
    }

    #[test]
    fn short_lines_are_untouched() {
        let points = ramp(1000);
        assert!(matches!(
            view_points(&points, None, 800),
            Cow::Borrowed(kept) if kept.len() == 1000
        ));
    }

    #[test]
    fn long_lines_shrink_but_keep_extremes_and_ends() {
        let mut points = ramp(190_000);
        points[100_001][1] = 9_999.0;
        points[150_000][1] = -9_999.0;
        let reduced = view_points(&points, None, 800);
        assert!(reduced.len() <= 800 * POINTS_PER_PIXEL);
        assert_eq!(reduced.first(), points.first());
        assert_eq!(reduced.last(), points.last());
        assert!(reduced.iter().any(|p| p[1] == 9_999.0));
        assert!(reduced.iter().any(|p| p[1] == -9_999.0));
        assert!(reduced.windows(2).all(|pair| pair[0][0] <= pair[1][0]));
    }

    #[test]
    fn zooming_in_returns_the_underlying_samples() {
        let points = ramp(190_000);
        let reduced = view_points(&points, Some([1_000.0, 1_200.0]), 800);
        // The view plus its margins hold only a few hundred samples, so none
        // are dropped.
        assert_eq!(reduced.len(), 403);
        assert!(reduced.iter().any(|p| p[0] == 1_100.0));
        assert!(reduced.first().unwrap()[0] <= 900.0);
        assert!(reduced.last().unwrap()[0] >= 1_300.0);
    }

    #[test]
    fn unordered_x_cannot_be_cut_to_a_view() {
        let mut points = ramp(190_000);
        points.swap(10, 20);
        let reduced = view_points(&points, Some([1_000.0, 1_200.0]), 800);
        assert!(reduced.len() <= 800 * POINTS_PER_PIXEL);
        assert_eq!(reduced.last(), points.last());
    }
}
