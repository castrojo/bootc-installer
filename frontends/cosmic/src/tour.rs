//! Tour carousel: the per-image slides shown last, before the summary.
//!
//! GNOME shows one `BootcTour` page per entry of the selected image's
//! `carousel` (from `images.json`, via `get_finals`), falling back to the
//! recipe's `tour` (`{"welcome": {...}, "done": {...}}`,
//! `bootc_installer/utils/recipe.py`). Each slide is a title, a description
//! and an asset (`image` path or `resource:///...` URI, as `tour.py`
//! accepts both). This module parses both shapes into `Slide`s and owns the
//! slide-index arithmetic; rendering stays in `ui.rs`.

use serde_json::Value;

/// One carousel slide.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Slide {
    pub title: String,
    pub description: String,
    /// Asset reference as carried (`image` path or `resource:///...` URI).
    pub image: String,
}

fn slide_from(value: &Value) -> Option<Slide> {
    let obj = value.as_object()?;
    let title = obj
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let description = obj
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    // `tour.py` reads `resource` first, then `image`.
    let image = obj
        .get("resource")
        .or_else(|| obj.get("image"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if title.is_empty() && description.is_empty() && image.is_empty() {
        return None;
    }
    Some(Slide {
        title,
        description,
        image,
    })
}

/// Slides from an images.json `carousel` list. Empty entries are dropped;
/// an empty list means the tour page is skipped.
pub fn slides_from_carousel(carousel: &[Value]) -> Vec<Slide> {
    carousel.iter().filter_map(slide_from).collect()
}

/// Whether a slide asset names a local file (renderable here) rather than
/// a GResource URI (a GNOME-packaging concept this frontend cannot read).
pub fn is_local_asset(asset: &str) -> bool {
    !asset.is_empty() && !asset.starts_with("resource:") && asset.starts_with('/')
}

/// Clamp a slide index into a list of `len` slides.
pub fn clamp_index(index: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        index.min(len - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn carousel_list_parses_with_either_asset_key() {
        let carousel = vec![
            json!({"title": "One", "description": "First", "image": "/a.png"}),
            json!({"title": "Two", "description": "Second", "resource": "resource:///org/x.svg"}),
            json!({}),
        ];
        let slides = slides_from_carousel(&carousel);
        assert_eq!(slides.len(), 2);
        assert_eq!(slides[0].image, "/a.png");
        assert_eq!(slides[1].image, "resource:///org/x.svg");
    }

    #[test]
    fn only_local_files_are_local_assets() {
        assert!(is_local_asset("/usr/share/x.png"));
        assert!(!is_local_asset("resource:///org/x.svg"));
        assert!(!is_local_asset("resource://image"));
        assert!(!is_local_asset(""));
        assert!(!is_local_asset("relative.png"));
    }

    #[test]
    fn index_clamping() {
        assert_eq!(clamp_index(0, 0), 0);
        assert_eq!(clamp_index(5, 0), 0);
        assert_eq!(clamp_index(5, 3), 2);
        assert_eq!(clamp_index(1, 3), 1);
    }
}
