//! Test helpers for crates that work on 2D shapes (kurbo paths):
//!
//! - [`metrics`]: compare two filled shapes on a raster (IoU, Hausdorff distance, boundary length);
//! - [`report`]: visual HTML/SVG test reports under `<target dir>/test-reports/`;
//! - [`data`]: test data kept as SVG files (viewable in a browser, exact coordinates).
//!
//! Nothing here knows where the data comes from: every crate keeps its own files under its own
//! `tests/data/`.

pub mod data;
pub mod metrics;
pub mod report;

use std::path::PathBuf;

use kurbo::{BezPath, Cap, Join, Stroke, StrokeOpts};

/// Where reports go: `<target dir>/test-reports`. The target directory is `CARGO_TARGET_DIR` when
/// set, otherwise the `target` directory the running test or example binary was built into.
pub fn reports_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
        return PathBuf::from(dir).join("test-reports");
    }
    let exe = std::env::current_exe().expect("path of the running executable");
    exe.ancestors()
        .find(|p| p.file_name().is_some_and(|n| n == "target"))
        .map(|target| target.join("test-reports"))
        .unwrap_or_else(|| std::env::temp_dir().join("test-reports"))
}

/// The area a constant-width line covers: the path stroked with round caps and joins.
pub fn stroked(path: &BezPath, width: f64) -> BezPath {
    let style = Stroke::new(width).with_caps(Cap::Round).with_join(Join::Round);
    kurbo::stroke(path.iter(), &style, &StrokeOpts::default(), 1e-4)
}
