//! Camera zoom: between two times, the video is shown enlarged around a
//! point (fixed, or the cursor), easing in and out.
//!
//! The editor's preview computes the same view (`zoomView` in
//! `app/src/editor/zoom.ts`).

use serde::{Deserialize, Serialize};

use crate::cursor::CursorTrack;

/// A zoom between two times (seconds of the source).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ZoomRegion {
    pub start: f64,
    pub end: f64,
    /// Enlargement at its height, e.g. 2.
    pub scale: f32,
    /// The point kept in view, in pixels of the source; `None` follows the
    /// cursor.
    #[serde(default)]
    pub focus: Option<[f32; 2]>,
}

/// Longest time a zoom takes to go in or out, in seconds.
const EASE: f64 = 0.7;
/// Stiffness of the camera following the cursor: much slower than the
/// cursor itself, so that it drifts rather than shakes.
pub const CAMERA_STIFFNESS: f64 = 3.5;

/// Ease in and out (cubic) of `u` in 0..=1.
fn ease(u: f64) -> f64 {
    let u = u.clamp(0.0, 1.0);
    if u < 0.5 {
        4.0 * u * u * u
    } else {
        1.0 - (-2.0 * u + 2.0).powi(3) / 2.0
    }
}

impl ZoomRegion {
    /// How far zoomed in at `t`: 0 outside, 1 in the middle.
    pub fn weight(&self, t: f64) -> f64 {
        if t <= self.start || t >= self.end {
            return 0.0;
        }
        let ramp = EASE.min((self.end - self.start) / 2.0).max(1e-3);
        ease((t - self.start) / ramp).min(ease((self.end - t) / ramp))
    }
}

/// The zooms of an edit, ready to tell what is in view at each time.
#[derive(Debug, Clone, Default)]
pub struct Zoom {
    regions: Vec<ZoomRegion>,
    /// The cursor, slowed down for the camera to follow.
    camera: Option<CursorTrack>,
}

impl Zoom {
    pub fn new(regions: &[ZoomRegion], track: Option<&CursorTrack>) -> Self {
        let follows = regions.iter().any(|r| r.focus.is_none());
        Self {
            regions: regions
                .iter()
                .filter(|r| r.end > r.start && r.scale > 1.0)
                .cloned()
                .collect(),
            camera: track
                .filter(|_| follows)
                .map(|t| t.smoothed(CAMERA_STIFFNESS)),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// The part of `crop` (`[x, y, w, h]`, source pixels) in view at `t`.
    #[allow(clippy::cast_possible_truncation)]
    pub fn view(&self, crop: [f32; 4], t: f64) -> [f32; 4] {
        let [cx, cy, cw, ch] = crop;
        let best = self
            .regions
            .iter()
            .map(|r| (r, r.weight(t)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        let Some((region, weight)) = best.filter(|(_, w)| *w > 0.0) else {
            return crop;
        };
        let scale = 1.0 + (region.scale.clamp(1.0, 6.0) - 1.0) * weight as f32;
        let (vw, vh) = (cw / scale, ch / scale);
        let [fx, fy] = region
            .focus
            .or_else(|| {
                let (x, y, _) = self.camera.as_ref()?.at(t)?;
                Some([x, y])
            })
            .unwrap_or([cx + cw / 2.0, cy + ch / 2.0]);
        let left = (fx - vw / 2.0).clamp(cx, cx + cw - vw);
        let top = (fy - vh / 2.0).clamp(cy, cy + ch - vh);
        [left, top, vw, vh]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(focus: Option<[f32; 2]>) -> ZoomRegion {
        ZoomRegion {
            start: 1.0,
            end: 4.0,
            scale: 2.0,
            focus,
        }
    }

    #[test]
    fn zooms_ease_in_and_out() {
        let r = region(None);
        assert!(r.weight(0.5).abs() < 1e-9);
        assert!(r.weight(1.2) > 0.0 && r.weight(1.2) < 0.5);
        assert!((r.weight(2.5) - 1.0).abs() < 1e-9);
        assert!(r.weight(4.0).abs() < 1e-9);
    }

    #[test]
    fn the_view_stays_inside_the_crop() {
        let zoom = Zoom::new(&[region(Some([5.0, 5.0]))], None);
        let crop = [0.0, 0.0, 200.0, 100.0];
        assert_eq!(zoom.view(crop, 0.0), crop);
        // Halfway through: half the size, pushed against the top-left.
        assert_eq!(zoom.view(crop, 2.5), [0.0, 0.0, 100.0, 50.0]);
        let centred = Zoom::new(&[region(Some([100.0, 50.0]))], None);
        assert_eq!(centred.view(crop, 2.5), [50.0, 25.0, 100.0, 50.0]);
    }
}

#[cfg(test)]
mod smoothing {
    use crate::cursor::{CursorSample, CursorTrack};

    #[test]
    fn a_smoothed_cursor_glides_to_where_it_went() {
        let at = |t: f64, x: f32| CursorSample {
            t,
            x,
            y: 0.0,
            shape: 0,
            visible: true,
        };
        // A jump from 0 to 100 at 0.5 s, then still until 2 s.
        let track = CursorTrack {
            samples: vec![at(0.0, 0.0), at(0.5, 0.0), at(0.501, 100.0), at(2.0, 100.0)],
            ..CursorTrack::default()
        };
        let smooth = track.smoothed(12.0);
        let x = |t: f64| smooth.at(t).map_or(f32::NAN, |p| p.0);
        assert!(x(0.52) < 30.0, "lags right after the jump");
        assert!(x(0.6) > 30.0 && x(0.6) < 100.0);
        assert!((x(2.0) - 100.0).abs() < 1.0, "arrives");
    }
}
