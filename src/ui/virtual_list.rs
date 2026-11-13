//! Windowing math for fixed-height rows inside a `scrollable`: render only the
//! rows near the viewport and pad the rest with two spacers.

pub struct Window {
    pub start: usize,
    pub end: usize,
    pub top: f32,
    pub bottom: f32,
}

pub fn window(total: usize, row_h: f32, scroll_y: f32, viewport_h: f32, overscan: usize) -> Window {
    let first = ((scroll_y.max(0.0) / row_h).floor() as usize)
        .saturating_sub(overscan)
        .min(total);
    let visible = (viewport_h / row_h).ceil() as usize + 1 + 2 * overscan;
    let end = (first + visible).min(total);
    Window {
        start: first,
        end,
        top: first as f32 * row_h,
        bottom: (total - end) as f32 * row_h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_covers_viewport_and_pads_the_rest() {
        let w = window(10_000, 20.0, 2_000.0, 400.0, 3);
        assert_eq!(w.start, 97);
        assert!(w.end >= 100 + 20, "must cover the viewport");
        let total_h = w.top + (w.end - w.start) as f32 * 20.0 + w.bottom;
        assert_eq!(total_h, 10_000.0 * 20.0);
    }

    #[test]
    fn small_lists_and_overscroll_are_safe() {
        let w = window(3, 20.0, 9_999.0, 400.0, 3);
        assert!(w.start <= w.end && w.end <= 3);
        let w = window(0, 20.0, 0.0, 400.0, 3);
        assert_eq!((w.start, w.end), (0, 0));
    }
}
