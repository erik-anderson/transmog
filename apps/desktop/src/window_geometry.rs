//! Physical-pixel geometry for recovering an inaccessible restored main window.

#[derive(Clone, Copy, Debug)]
pub(crate) struct PhysicalBounds {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl PhysicalBounds {
    fn has_area(self) -> bool {
        self.width > 0 && self.height > 0
    }

    fn overlaps(self, other: Self) -> bool {
        // A saved coordinate plus a physical size can exceed i32, even when
        // both values are valid independently. Compare widened half-open bounds.
        self.has_area()
            && other.has_area()
            && i64::from(self.x) < i64::from(other.x) + i64::from(other.width)
            && i64::from(other.x) < i64::from(self.x) + i64::from(self.width)
            && i64::from(self.y) < i64::from(other.y) + i64::from(other.height)
            && i64::from(other.y) < i64::from(self.y) + i64::from(self.height)
    }
}

/// Select a recovery monitor only when no part of the normal outer bounds is visible.
pub(crate) fn recovery_monitor(
    window: PhysicalBounds,
    monitors: &[PhysicalBounds],
    primary: Option<usize>,
) -> Option<usize> {
    if !window.has_area() || monitors.iter().any(|monitor| window.overlaps(*monitor)) {
        return None;
    }
    primary
        .filter(|index| {
            monitors
                .get(*index)
                .is_some_and(|monitor| monitor.has_area())
        })
        .or_else(|| monitors.iter().position(|monitor| monitor.has_area()))
}

#[cfg(test)]
mod tests {
    use super::{PhysicalBounds, recovery_monitor};

    fn bounds(x: i32, y: i32, width: u32, height: u32) -> PhysicalBounds {
        PhysicalBounds {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn preserves_visible_windows_on_negative_coordinate_displays() {
        let monitors = [bounds(0, 0, 1920, 1080), bounds(-2560, -1440, 2560, 1440)];
        for window in [
            bounds(100, 100, 960, 720),
            bounds(-2400, -1300, 1200, 800),
            bounds(-10, -10, 100, 100),
        ] {
            assert_eq!(recovery_monitor(window, &monitors, Some(0)), None);
        }
    }

    #[test]
    fn preserves_even_one_pixel_of_visible_outer_bounds() {
        let monitor = bounds(0, 0, 1920, 1080);
        for window in [
            bounds(-959, 100, 960, 720),
            bounds(1919, 100, 960, 720),
            bounds(100, -719, 960, 720),
            bounds(100, 1079, 960, 720),
            bounds(-959, -719, 960, 720),
            // Visibility is measured against the whole display, including the
            // taskbar strip, rather than only its work area.
            bounds(100, 1070, 960, 10),
        ] {
            assert_eq!(recovery_monitor(window, &[monitor], Some(0)), None);
        }
    }

    #[test]
    fn recovers_completely_offscreen_windows_in_every_direction() {
        let monitors = [bounds(0, 0, 1920, 1080)];
        for window in [
            bounds(-960, 100, 960, 720),
            bounds(1920, 100, 960, 720),
            bounds(100, -720, 960, 720),
            bounds(100, 1080, 960, 720),
            bounds(30_000, 30_000, 960, 720),
        ] {
            assert_eq!(recovery_monitor(window, &monitors, Some(0)), Some(0));
        }
    }

    #[test]
    fn recovers_windows_in_gaps_between_displays() {
        let monitors = [bounds(-1920, 0, 1920, 1080), bounds(0, 1440, 2560, 1440)];
        assert_eq!(
            recovery_monitor(bounds(100, 100, 960, 720), &monitors, Some(1)),
            Some(1)
        );
    }

    #[test]
    fn does_not_move_a_window_without_a_usable_monitor_inventory() {
        let window = bounds(30_000, 30_000, 960, 720);
        assert_eq!(recovery_monitor(window, &[], Some(0)), None);
        assert_eq!(
            recovery_monitor(window, &[bounds(0, 0, 0, 1080)], None),
            None
        );
    }

    #[test]
    fn falls_back_to_the_first_usable_monitor_without_a_primary() {
        let window = bounds(30_000, 30_000, 960, 720);
        let monitors = [bounds(0, 0, 0, 1080), bounds(-1920, 0, 1920, 1080)];
        for primary in [None, Some(0), Some(10)] {
            assert_eq!(recovery_monitor(window, &monitors, primary), Some(1));
        }
    }

    #[test]
    fn handles_extreme_physical_coordinates_and_sizes_without_overflow() {
        let monitor = bounds(i32::MAX - 100, i32::MAX - 100, 200, 200);
        let window = bounds(i32::MAX - 1, i32::MAX - 1, u32::MAX, u32::MAX);
        assert_eq!(recovery_monitor(window, &[monitor], None), None);

        let monitor = bounds(i32::MIN, i32::MIN, u32::MAX, u32::MAX);
        assert_eq!(recovery_monitor(bounds(0, 0, 1, 1), &[monitor], None), None);
        assert_eq!(
            recovery_monitor(
                bounds(i32::MIN, i32::MIN, 960, 720),
                &[bounds(0, 0, 1920, 1080)],
                None,
            ),
            Some(0)
        );
    }
}
