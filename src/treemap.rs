//! Squarified treemap layout (Bruls, Huizing & van Wijk, 2000).

use eframe::egui::{Rect, pos2, vec2};

pub struct Layout {
    /// One rectangle per laid-out item, in input order.
    pub rects: Vec<Rect>,
    /// Space for the remaining items that were too small to draw
    /// individually, lumped into one block.
    pub rest: Option<Rect>,
}

/// Lays out `sizes` (sorted largest first) inside `bounds`.
///
/// Items whose share of the area would fall below `min_area` square pixels
/// are not laid out one by one; their combined area becomes `rest`. This
/// keeps folders holding hundreds of thousands of tiny files cheap to draw.
pub fn squarify(sizes: &[u64], bounds: Rect, min_area: f32) -> Layout {
    let total: f64 = sizes.iter().map(|&s| s as f64).sum();
    let area = (bounds.width() * bounds.height()) as f64;
    if total <= 0.0 || area <= 0.0 {
        return Layout { rects: Vec::new(), rest: None };
    }
    let scale = area / total;
    let shown = sizes.iter().take_while(|&&s| s as f64 * scale >= min_area as f64).count();
    let mut areas: Vec<f64> = sizes[..shown].iter().map(|&s| s as f64 * scale).collect();
    let rest_area: f64 = sizes[shown..].iter().map(|&s| s as f64 * scale).sum();
    let has_rest = rest_area > 0.0;
    if has_rest {
        // Lay the lump out as a final pseudo-item so it gets real space.
        areas.push(rest_area);
    }

    let mut rects = Vec::with_capacity(areas.len());
    let (mut x, mut y, mut w, mut h) =
        (bounds.min.x as f64, bounds.min.y as f64, bounds.width() as f64, bounds.height() as f64);
    let mut i = 0;
    while i < areas.len() {
        let side = w.min(h);
        // Grow the row while it improves the worst aspect ratio.
        let mut end = i + 1;
        let mut row_sum = areas[i];
        let mut best = worst_ratio(&areas[i..end], row_sum, side);
        while end < areas.len() {
            let next_sum = row_sum + areas[end];
            let ratio = worst_ratio(&areas[i..=end], next_sum, side);
            if ratio > best {
                break;
            }
            best = ratio;
            row_sum = next_sum;
            end += 1;
        }
        let last_row = end == areas.len();
        if w >= h {
            // Column along the left edge.
            let col_w = if last_row { w } else { row_sum / h };
            let mut cy = y;
            for (k, &a) in areas[i..end].iter().enumerate() {
                let ih = if k == end - i - 1 { y + h - cy } else { a / col_w };
                rects.push(make_rect(x, cy, col_w, ih));
                cy += ih;
            }
            x += col_w;
            w -= col_w;
        } else {
            // Row along the top edge.
            let row_h = if last_row { h } else { row_sum / w };
            let mut cx = x;
            for (k, &a) in areas[i..end].iter().enumerate() {
                let iw = if k == end - i - 1 { x + w - cx } else { a / row_h };
                rects.push(make_rect(cx, y, iw, row_h));
                cx += iw;
            }
            y += row_h;
            h -= row_h;
        }
        i = end;
    }

    let rest = if has_rest { rects.pop() } else { None };
    Layout { rects, rest }
}

fn make_rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect::from_min_size(pos2(x as f32, y as f32), vec2(w.max(0.0) as f32, h.max(0.0) as f32))
}

/// Worst aspect ratio of a row of areas laid along a side of length `side`.
fn worst_ratio(row: &[f64], sum: f64, side: f64) -> f64 {
    let (mut max, mut min) = (f64::MIN, f64::MAX);
    for &a in row {
        max = max.max(a);
        min = min.min(a);
    }
    let s2 = sum * sum;
    let w2 = side * side;
    (w2 * max / s2).max(s2 / (w2 * min))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> Rect {
        Rect::from_min_size(pos2(10.0, 20.0), vec2(600.0, 400.0))
    }

    #[test]
    fn areas_are_proportional_and_tile_the_bounds() {
        let sizes = [600, 300, 200, 100, 50, 30, 20];
        let layout = squarify(&sizes, bounds(), 0.0);
        assert_eq!(layout.rects.len(), sizes.len());
        assert!(layout.rest.is_none());
        let total: u64 = sizes.iter().sum();
        let area = 600.0 * 400.0;
        let mut covered = 0.0;
        for (r, &s) in layout.rects.iter().zip(&sizes) {
            let expected = s as f32 / total as f32 * area;
            assert!((r.area() - expected).abs() < 1.0, "{r:?} vs {expected}");
            assert!(bounds().expand(0.01).contains_rect(*r));
            covered += r.area();
        }
        assert!((covered - area).abs() < 1.0);
    }

    #[test]
    fn rects_do_not_overlap() {
        let sizes: Vec<u64> = (1..40).rev().map(|n| n * n).collect();
        let layout = squarify(&sizes, bounds(), 0.0);
        for (a, ra) in layout.rects.iter().enumerate() {
            for rb in &layout.rects[a + 1..] {
                assert!(ra.intersect(*rb).area() < 0.01, "{ra:?} overlaps {rb:?}");
            }
        }
    }

    #[test]
    fn aspect_ratios_stay_reasonable() {
        let sizes = [10; 16];
        let layout = squarify(&sizes, bounds(), 0.0);
        for r in &layout.rects {
            let ratio = r.width().max(r.height()) / r.width().min(r.height());
            assert!(ratio < 2.5, "aspect ratio {ratio}");
        }
    }

    #[test]
    fn tiny_items_are_lumped() {
        let mut sizes = vec![1_000_000u64, 500_000];
        sizes.extend(std::iter::repeat_n(1, 10_000));
        let layout = squarify(&sizes, bounds(), 4.0);
        assert_eq!(layout.rects.len(), 2);
        let rest = layout.rest.expect("rest block");
        assert!(rest.area() > 0.0);
    }

    #[test]
    fn empty_input() {
        assert!(squarify(&[], bounds(), 1.0).rects.is_empty());
        assert!(squarify(&[0, 0], bounds(), 1.0).rects.is_empty());
    }
}
