//! SVG bounding box calculation.
//!
//! Calculates the true visual bounds of SVG content including stroke width.
//! This prevents content clipping when converting SVG to external files.

use usvg::{Node, Rect, Tree};

/// Calculate the stroke-inclusive bounding box of all elements in the SVG tree
///
/// This iterates through all nodes and computes the union of their stroke bounding boxes
/// The result includes the full stroke width, preventing clipping at edges
///
/// # Returns
/// - `Some(Rect)` - The combined bounding box of all visible elements
/// - `None` - If the tree has no visible elements
pub fn calculate_stroke_bounds(tree: &Tree) -> Option<Rect> {
    let mut bounds: Option<Rect> = None;

    // Recursively traverse all nodes starting from root
    traverse_group(tree.root(), &mut bounds);

    bounds
}

/// Recursively traverse a group and its children, accumulating bounds
fn traverse_group(group: &usvg::Group, bounds: &mut Option<Rect>) {
    for node in group.children() {
        let node_bounds = node.stroke_bounding_box();
        *bounds = merge_bounds(*bounds, node_bounds);

        // Recursively traverse nested groups
        if let Node::Group(nested_group) = node {
            traverse_group(nested_group, bounds);
        }
    }
}

/// Merge two optional bounding boxes into one
fn merge_bounds(a: Option<Rect>, b: Rect) -> Option<Rect> {
    match a {
        Some(existing) => {
            // Calculate union of two rectangles
            let min_x = existing.x().min(b.x());
            let min_y = existing.y().min(b.y());
            let max_x = existing.right().max(b.right());
            let max_y = existing.bottom().max(b.bottom());

            Rect::from_xywh(min_x, min_y, max_x - min_x, max_y - min_y)
        }
        None => Some(b),
    }
}

/// Expand the viewBox of an SVG string to match the given bounds
///
/// # Arguments
/// * `svg` - The SVG string to modify
/// * `bounds` - The new bounding box to use as viewBox
///
/// # Returns
/// The SVG string with updated viewBox attribute
pub fn expand_viewbox_to_bounds(svg: &str, bounds: Rect) -> String {
    let new_viewbox = format!(
        "{} {} {} {}",
        bounds.x(),
        bounds.y(),
        bounds.width(),
        bounds.height()
    );

    replace_viewbox(svg, &new_viewbox)
}

/// Replace the viewBox attribute in an SVG string
fn replace_viewbox(svg: &str, new_viewbox: &str) -> String {
    if let Some(start) = svg.find("viewBox=\"") {
        let attr_start = start + 9; // len of 'viewBox="'
        if let Some(end) = svg[attr_start..].find('"') {
            return format!(
                "{}viewBox=\"{}\"{}",
                &svg[..start],
                new_viewbox,
                &svg[attr_start + end + 1..]
            );
        }
    }

    // No viewBox found, add one after <svg
    if let Some(svg_tag_end) = svg.find("<svg") {
        let insert_pos = svg_tag_end + 4;
        // Find the end of the opening tag attributes
        if let Some(space_or_gt) = svg[insert_pos..].find([' ', '>']) {
            let insert_pos = insert_pos + space_or_gt;
            return format!(
                "{} viewBox=\"{}\"{}",
                &svg[..insert_pos],
                new_viewbox,
                &svg[insert_pos..]
            );
        }
    }

    svg.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replace_viewbox_existing() {
        let svg = r#"<svg viewBox="0 0 100 100" width="100">"#;
        let result = replace_viewbox(svg, "-5 -5 110 110");
        assert_eq!(result, r#"<svg viewBox="-5 -5 110 110" width="100">"#);
    }

    #[test]
    fn test_replace_viewbox_no_existing() {
        let svg = r#"<svg width="100" height="100">"#;
        let result = replace_viewbox(svg, "0 0 100 100");
        assert!(result.contains("viewBox=\"0 0 100 100\""));
    }

    #[test]
    fn test_merge_bounds() {
        let a = Rect::from_xywh(0.0, 0.0, 100.0, 100.0);
        let b = Rect::from_xywh(-10.0, -10.0, 50.0, 50.0).unwrap();

        let merged = merge_bounds(a, b);
        assert!(merged.is_some());

        let m = merged.unwrap();
        assert_eq!(m.x(), -10.0);
        assert_eq!(m.y(), -10.0);
        assert_eq!(m.right(), 100.0);
        assert_eq!(m.bottom(), 100.0);
    }
}
