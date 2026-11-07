//! Visual test reports: self-contained HTML pages with inline SVG panels.
//!
//! Every report is written to `<target dir>/test-reports/<name>.html` (see [`crate::reports_dir`]),
//! and `index.html` in the same directory is regenerated to link all reports.

use std::fmt::Write as _;
use std::path::PathBuf;

use kurbo::{BezPath, Point, Rect};

/// Colors used consistently across reports.
pub mod color {
    /// Original Goodnotes outline (filled).
    pub const ORIGINAL: &str = "#c9c9c9";
    /// Regenerated outline (stroked on top of the original).
    pub const REGENERATED: &str = "#e0245e";
    /// Regenerated outline fill, semi-transparent.
    pub const REGENERATED_FILL: &str = "rgba(224,36,94,0.25)";
    /// Skeleton centerline.
    pub const SPINE: &str = "#1d4ed8";
    /// Skeleton sample circles.
    pub const CIRCLE: &str = "rgba(29,78,216,0.55)";
    pub const START: &str = "#16a34a";
    pub const END: &str = "#dc2626";
}

/// An SVG panel whose view box is in page points; line widths and text sizes are given in screen
/// pixels and converted to page points (no `vector-effect`, so every SVG renderer agrees).
pub struct Svg {
    view: Rect,
    width_px: f64,
    body: String,
}

impl Svg {
    pub fn new(view: Rect, width_px: f64) -> Self {
        Self { view, width_px, body: String::new() }
    }

    /// Page points per screen pixel.
    fn unit(&self) -> f64 {
        self.view.width().max(1e-9) / self.width_px
    }

    pub fn path(&mut self, path: &BezPath, fill: &str, stroke: &str, stroke_px: f64) {
        let _ = writeln!(
            self.body,
            r#"<path d="{}" fill="{fill}" fill-rule="nonzero" stroke="{stroke}" stroke-width="{:.5}"/>"#,
            path.to_svg(),
            stroke_px * self.unit()
        );
    }

    pub fn polyline(&mut self, pts: &[Point], stroke: &str, stroke_px: f64) {
        let pts: Vec<String> = pts.iter().map(|p| format!("{:.4},{:.4}", p.x, p.y)).collect();
        let _ = writeln!(
            self.body,
            r#"<polyline points="{}" fill="none" stroke="{stroke}" stroke-width="{:.5}"/>"#,
            pts.join(" "),
            stroke_px * self.unit()
        );
    }

    /// A circle with a radius in page points.
    pub fn circle(&mut self, c: Point, r: f64, stroke: &str, stroke_px: f64) {
        let _ = writeln!(
            self.body,
            r#"<circle cx="{:.4}" cy="{:.4}" r="{:.4}" fill="none" stroke="{stroke}" stroke-width="{:.5}"/>"#,
            c.x, c.y, r, stroke_px * self.unit()
        );
    }

    /// A dot with a radius in screen pixels.
    pub fn dot(&mut self, c: Point, r_px: f64, fill: &str) {
        let r = r_px * self.unit();
        let _ = writeln!(self.body, r#"<circle cx="{:.4}" cy="{:.4}" r="{r:.4}" fill="{fill}"/>"#, c.x, c.y);
    }

    /// Text at a page position, with a size in screen pixels.
    pub fn text(&mut self, at: Point, text: &str, size_px: f64, fill: &str) {
        let size = size_px * self.unit();
        let _ = writeln!(
            self.body,
            r#"<text x="{:.4}" y="{:.4}" font-size="{size:.4}" fill="{fill}" font-family="sans-serif">{}</text>"#,
            at.x,
            at.y,
            escape(text)
        );
    }

    pub fn render(&self) -> String {
        let v = self.view;
        let height_px = self.width_px * v.height() / v.width().max(1e-9);
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{} {} {} {}" width="{:.0}" height="{:.0}">{}</svg>"#,
            v.x0,
            v.y0,
            v.width(),
            v.height(),
            self.width_px,
            height_px,
            self.body
        )
    }
}

pub struct Panel {
    pub caption: String,
    pub svg: String,
    /// Highlight the panel (e.g. a metric is out of range).
    pub flagged: bool,
}

pub struct Section {
    pub heading: String,
    pub note: String,
    pub panels: Vec<Panel>,
}

pub struct Report {
    name: String,
    title: String,
    summary: Vec<String>,
    sections: Vec<Section>,
}

impl Report {
    pub fn new(name: &str, title: &str) -> Self {
        Self { name: name.into(), title: title.into(), summary: Vec::new(), sections: Vec::new() }
    }

    /// A line in the summary block at the top.
    pub fn summary(&mut self, line: impl Into<String>) {
        self.summary.push(line.into());
    }

    pub fn section(&mut self, heading: impl Into<String>, note: impl Into<String>) -> &mut Section {
        self.sections.push(Section { heading: heading.into(), note: note.into(), panels: Vec::new() });
        self.sections.last_mut().unwrap()
    }

    /// Writes `target/test-reports/<name>.html` and refreshes `index.html`. Returns the report path.
    pub fn write(&self) -> PathBuf {
        let dir = super::reports_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let mut html = String::new();
        let _ = write!(html, "{HEAD}<title>{}</title></head><body><h1>{}</h1>", escape(&self.title), escape(&self.title));
        if !self.summary.is_empty() {
            html += "<pre class=summary>";
            for line in &self.summary {
                html += &escape(line);
                html += "\n";
            }
            html += "</pre>";
        }
        html += LEGEND;
        for s in &self.sections {
            let _ = write!(html, "<h2>{}</h2><p class=note>{}</p><div class=grid>", escape(&s.heading), escape(&s.note));
            for p in &s.panels {
                let class = if p.flagged { "panel flagged" } else { "panel" };
                let _ = write!(html, "<figure class=\"{class}\">{}<figcaption>{}</figcaption></figure>", p.svg, escape(&p.caption));
            }
            html += "</div>";
        }
        html += "</body></html>";
        // Every panel also as a standalone SVG: `<name>/<section>-<panel>.svg`.
        let panel_dir = dir.join(&self.name);
        let _ = std::fs::remove_dir_all(&panel_dir);
        std::fs::create_dir_all(&panel_dir).unwrap();
        for (i, s) in self.sections.iter().enumerate() {
            for (j, p) in s.panels.iter().enumerate() {
                std::fs::write(panel_dir.join(format!("{i:02}-{j:03}.svg")), &p.svg).unwrap();
            }
        }
        let path = dir.join(format!("{}.html", self.name));
        std::fs::write(&path, html).unwrap();
        write_index(&dir);
        path
    }
}

fn write_index(dir: &std::path::Path) {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(".html") && n != "index.html")
        .collect();
    names.sort();
    let mut html = format!("{HEAD}<title>Test reports</title></head><body><h1>Test reports</h1><ul>");
    for n in names {
        let _ = write!(html, "<li><a href=\"{n}\">{}</a></li>", n.trim_end_matches(".html"));
    }
    html += "</ul></body></html>";
    // Several test binaries may write at the same time; a failed write only loses the index.
    let _ = std::fs::write(dir.join("index.html"), html);
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

const HEAD: &str = r#"<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<style>
:root{color-scheme:light dark;--bg:#fafaf7;--fg:#1f2937;--muted:#6b7280;--card:#fff;--line:#e5e7eb;--flag:#dc2626}
@media (prefers-color-scheme:dark){:root{--bg:#18181b;--fg:#e5e7eb;--muted:#9ca3af;--card:#f7f6ee;--line:#3f3f46}}
body{background:var(--bg);color:var(--fg);font:14px/1.5 system-ui,sans-serif;margin:24px}
h1{font-size:20px}h2{font-size:16px;margin-top:28px}.note{color:var(--muted);margin-top:-8px}
.summary{background:var(--card);color:#1f2937;padding:12px;border:1px solid var(--line);border-radius:6px;overflow:auto}
.grid{display:flex;flex-wrap:wrap;gap:12px}
.panel{margin:0;background:var(--card);border:1px solid var(--line);border-radius:6px;padding:6px;max-width:100%}
.panel svg{display:block;max-width:100%;height:auto}
.panel figcaption{color:#4b5563;font-size:12px;max-width:420px;white-space:pre-wrap}
.flagged{border:2px solid var(--flag)}
.legend span{display:inline-block;margin-right:14px}.legend i{display:inline-block;width:12px;height:12px;border-radius:2px;margin-right:4px;vertical-align:-1px}
a{color:#2563eb}
</style>"#;

const LEGEND: &str = r#"<p class=legend><span><i style="background:#c9c9c9"></i>original outline</span><span><i style="background:#e0245e"></i>regenerated outline</span><span><i style="background:#1d4ed8"></i>skeleton centerline / circles</span><span><i style="background:#16a34a"></i>pen down</span><span><i style="background:#dc2626"></i>pen up</span><span><i style="border:2px solid #dc2626;background:none"></i>metric out of range</span></p>"#;
