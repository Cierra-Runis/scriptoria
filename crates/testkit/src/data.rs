//! Test data as SVG files.
//!
//! An SVG file opens in any browser, so test data can be looked at directly, and kurbo writes path
//! data at full `f64` precision, so nothing is lost. The format is a small, fixed subset of SVG
//! that this module writes and reads back:
//!
//! ```text
//! <svg …>
//!   <g id="佳" data-note="…">          optional group, e.g. the strokes of one character in order
//!     <path id="…" data-kind="outline" data-tags="hook tip" data-source="…" d="…"/>
//!     <path id="…" data-kind="line" data-width="0.25" d="…"/>
//!   </g>
//! </svg>
//! ```
//!
//! A skeleton (circles along a centerline) is a `<g class="skeleton" id="…">` holding one
//! `<path class="piece" data-r="r₀ r₁ …" d="…"/>` per connected piece: the path is the centerline,
//! one cubic segment between consecutive circle centers (a lone `M x y` for a single circle), and
//! `data-r` lists the radii. Circles drawn for viewing are ignored when reading.

use std::fmt::Write as _;
use std::path::Path;

use kurbo::{BezPath, CubicBez, PathEl, Point, Rect, Shape};

/// The items of one group, in file order. Items outside any `<g>` form groups with `id: None`.
#[derive(Clone, Debug)]
pub struct Group<T> {
    pub id: Option<String>,
    pub note: Option<String>,
    pub items: Vec<T>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    pub id: String,
    pub tags: Vec<String>,
    /// Where the stroke was taken from (free text, for people).
    pub source: Option<String>,
    pub kind: StrokeKind,
    pub path: BezPath,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StrokeKind {
    /// A filled outline (nonzero fill rule).
    Outline,
    /// A constant-width line with round caps and joins.
    Line { width: f64 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Skeleton {
    pub id: String,
    pub tags: Vec<String>,
    pub source: Option<String>,
    pub pieces: Vec<SkeletonPiece>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkeletonPiece {
    /// Circle centers and radii, in order along the centerline.
    pub circles: Vec<(Point, f64)>,
    /// `spine[i]` runs from circle `i` to circle `i + 1`.
    pub spine: Vec<CubicBez>,
}

impl Stroke {
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }
}

impl Skeleton {
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }

    /// The region the skeleton describes at radius scale `scale`: see [`SkeletonPiece::swept`].
    pub fn swept(&self, scale: f64) -> BezPath {
        let mut out = BezPath::new();
        for piece in &self.pieces {
            out.extend(piece.swept(scale));
        }
        out
    }

    pub fn max_radius(&self) -> f64 {
        self.pieces.iter().flat_map(|p| &p.circles).map(|&(_, r)| r).fold(0.0, f64::max)
    }
}

impl SkeletonPiece {
    /// The region a circle sweeps moving along the centerline, its radius changing linearly in the
    /// curve parameter from one circle to the next and scaled by `scale`: a union of circles close
    /// enough (a fifth of the radius) that the notches between them stay below 0.5% of it.
    pub fn swept(&self, scale: f64) -> BezPath {
        use kurbo::{Circle, ParamCurve, ParamCurveArclen};
        let mut out = BezPath::new();
        let mut circle = |c: Point, r: f64| out.extend(Circle::new(c, r).path_elements(1e-3 * r.max(1e-6)));
        if self.spine.is_empty() {
            for &(c, r) in &self.circles {
                circle(c, r * scale);
            }
            return out;
        }
        for (i, seg) in self.spine.iter().enumerate() {
            let (r0, r1) = (self.circles[i].1 * scale, self.circles[i + 1].1 * scale);
            let steps = (seg.arclen(1e-5) / (0.2 * r0.min(r1).max(1e-5))).ceil().clamp(1.0, 4000.0) as usize;
            for k in 0..=steps {
                let t = k as f64 / steps as f64;
                circle(seg.eval(t), r0 + (r1 - r0) * t);
            }
        }
        out
    }
}

// ---- Writing ----

const INK: &str = "#1f2937";

pub fn write_strokes(path: &Path, title: &str, groups: &[Group<Stroke>]) {
    let mut bounds: Option<Rect> = None;
    let mut body = String::new();
    for group in groups {
        open_group(&mut body, group);
        for s in &group.items {
            let pad = match s.kind {
                StrokeKind::Outline => 0.0,
                StrokeKind::Line { width } => width / 2.0,
            };
            let b = s.path.bounding_box().inflate(pad, pad);
            bounds = Some(bounds.map_or(b, |a| a.union(b)));
            let _ = write!(body, "<path id=\"{}\"{}", escape(&s.id), common_attrs(&s.tags, &s.source));
            match s.kind {
                StrokeKind::Outline => {
                    let _ = write!(body, " data-kind=\"outline\" fill=\"{INK}\"");
                }
                StrokeKind::Line { width } => {
                    let _ = write!(
                        body,
                        " data-kind=\"line\" data-width=\"{width}\" fill=\"none\" stroke=\"{INK}\" stroke-width=\"{width}\" stroke-linecap=\"round\" stroke-linejoin=\"round\""
                    );
                }
            }
            let _ = writeln!(body, " d=\"{}\"/>", s.path.to_svg());
        }
        close_group(&mut body, group);
    }
    write_svg(path, title, bounds.unwrap_or(Rect::ZERO), &body);
}

pub fn write_skeletons(path: &Path, title: &str, groups: &[Group<Skeleton>]) {
    let mut bounds: Option<Rect> = None;
    let mut body = String::new();
    for group in groups {
        open_group(&mut body, group);
        for sk in &group.items {
            let _ = writeln!(body, "<g class=\"skeleton\" id=\"{}\"{}>", escape(&sk.id), common_attrs(&sk.tags, &sk.source));
            for piece in &sk.pieces {
                for &(c, r) in &piece.circles {
                    let b = Rect::from_center_size(c, (2.0 * r, 2.0 * r));
                    bounds = Some(bounds.map_or(b, |a| a.union(b)));
                    let _ = writeln!(body, "<circle cx=\"{}\" cy=\"{}\" r=\"{r}\" fill=\"#93c5fd55\" stroke=\"#1d4ed8\" stroke-width=\"0.005\"/>", c.x, c.y);
                }
                let radii: Vec<String> = piece.circles.iter().map(|(_, r)| r.to_string()).collect();
                let _ = writeln!(
                    body,
                    "<path class=\"piece\" data-r=\"{}\" fill=\"none\" stroke=\"#1d4ed8\" stroke-width=\"0.01\" d=\"{}\"/>",
                    radii.join(" "),
                    spine_path(piece).to_svg()
                );
            }
            body += "</g>\n";
        }
        close_group(&mut body, group);
    }
    write_svg(path, title, bounds.unwrap_or(Rect::ZERO), &body);
}

fn spine_path(piece: &SkeletonPiece) -> BezPath {
    let mut path = BezPath::new();
    if let Some(&(c, _)) = piece.circles.first() {
        path.move_to(c);
    }
    for seg in &piece.spine {
        path.curve_to(seg.p1, seg.p2, seg.p3);
    }
    path
}

fn common_attrs(tags: &[String], source: &Option<String>) -> String {
    let mut s = String::new();
    if !tags.is_empty() {
        let _ = write!(s, " data-tags=\"{}\"", escape(&tags.join(" ")));
    }
    if let Some(src) = source {
        let _ = write!(s, " data-source=\"{}\"", escape(src));
    }
    s
}

fn open_group<T>(body: &mut String, group: &Group<T>) {
    if let Some(id) = &group.id {
        let _ = write!(body, "<g id=\"{}\"", escape(id));
        if let Some(note) = &group.note {
            let _ = write!(body, " data-note=\"{}\"", escape(note));
        }
        body.push_str(">\n");
    }
}

fn close_group<T>(body: &mut String, group: &Group<T>) {
    if group.id.is_some() {
        body.push_str("</g>\n");
    }
}

fn write_svg(path: &Path, title: &str, bounds: Rect, body: &str) {
    let view = bounds.inflate(0.5, 0.5);
    let scale = 1600.0 / view.width().max(view.height()).max(1e-9);
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{} {} {} {}\" width=\"{:.0}\" height=\"{:.0}\">\n<title>{}</title>\n<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#fffef7\"/>\n{body}</svg>\n",
        view.x0,
        view.y0,
        view.width(),
        view.height(),
        view.width() * scale,
        view.height() * scale,
        escape(title),
        view.x0,
        view.y0,
        view.width(),
        view.height()
    );
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(path, svg).unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

// ---- Reading ----

pub fn read_strokes(path: &Path) -> Vec<Group<Stroke>> {
    let text = read(path);
    let mut groups: Vec<Group<Stroke>> = Vec::new();
    let mut open = false;
    for tag in tags(&text) {
        match (tag.name.as_str(), tag.close) {
            ("g", false) => {
                groups.push(Group { id: tag.get("id"), note: tag.get("data-note"), items: Vec::new() });
                open = true;
            }
            ("g", true) => open = false,
            ("path", false) => {
                let Some(kind) = tag.get("data-kind") else { continue };
                let kind = match kind.as_str() {
                    "outline" => StrokeKind::Outline,
                    "line" => StrokeKind::Line { width: tag.number("data-width") },
                    other => panic!("{}: unknown data-kind {other:?}", path.display()),
                };
                let stroke = Stroke {
                    id: tag.get("id").unwrap_or_default(),
                    tags: tag.list("data-tags"),
                    source: tag.get("data-source"),
                    kind,
                    path: tag.path(),
                };
                target(&mut groups, open).items.push(stroke);
            }
            _ => {}
        }
    }
    groups
}

pub fn read_skeletons(path: &Path) -> Vec<Group<Skeleton>> {
    let text = read(path);
    let mut groups: Vec<Group<Skeleton>> = Vec::new();
    let mut open = false;
    let mut in_skeleton = false;
    for tag in tags(&text) {
        match (tag.name.as_str(), tag.close) {
            ("g", false) if tag.get("class").as_deref() == Some("skeleton") => {
                let sk = Skeleton { id: tag.get("id").unwrap_or_default(), tags: tag.list("data-tags"), source: tag.get("data-source"), pieces: Vec::new() };
                target(&mut groups, open).items.push(sk);
                in_skeleton = true;
            }
            ("g", false) => {
                groups.push(Group { id: tag.get("id"), note: tag.get("data-note"), items: Vec::new() });
                open = true;
            }
            ("g", true) if in_skeleton => in_skeleton = false,
            ("g", true) => open = false,
            ("path", false) if in_skeleton && tag.get("class").as_deref() == Some("piece") => {
                let radii: Vec<f64> = tag.list("data-r").iter().map(|r| r.parse().expect("radius")).collect();
                let centerline = tag.path();
                let mut piece = SkeletonPiece::default();
                let mut last = Point::ORIGIN;
                for el in centerline.elements() {
                    match *el {
                        PathEl::MoveTo(p) => last = p,
                        PathEl::CurveTo(c1, c2, p) => {
                            piece.spine.push(CubicBez::new(last, c1, c2, p));
                            last = p;
                        }
                        ref other => panic!("{}: unexpected centerline element {other:?}", path.display()),
                    }
                }
                let centers = std::iter::once(piece.spine.first().map_or(last, |s| s.p0)).chain(piece.spine.iter().map(|s| s.p3));
                piece.circles = centers.zip(radii.iter().copied()).collect();
                assert_eq!(piece.circles.len(), radii.len(), "{}: radii and centerline do not match", path.display());
                let sk = groups.last_mut().and_then(|g| g.items.last_mut()).expect("piece outside a skeleton");
                sk.pieces.push(piece);
            }
            _ => {}
        }
    }
    groups
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// The group new items go to: the open `<g>`, or a group without id for items outside any.
fn target<T>(groups: &mut Vec<Group<T>>, open: bool) -> &mut Group<T> {
    if !open && groups.last().is_none_or(|g| g.id.is_some()) {
        groups.push(Group { id: None, note: None, items: Vec::new() });
    }
    groups.last_mut().unwrap()
}

struct Tag {
    name: String,
    close: bool,
    attrs: Vec<(String, String)>,
}

impl Tag {
    fn get(&self, key: &str) -> Option<String> {
        self.attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    fn list(&self, key: &str) -> Vec<String> {
        self.get(key).map(|v| v.split_whitespace().map(str::to_string).collect()).unwrap_or_default()
    }

    fn number(&self, key: &str) -> f64 {
        self.get(key).and_then(|v| v.parse().ok()).unwrap_or_else(|| panic!("<{}> needs a numeric {key}", self.name))
    }

    fn path(&self) -> BezPath {
        let d = self.get("d").unwrap_or_default();
        BezPath::from_svg(&d).unwrap_or_else(|e| panic!("bad path data {d:?}: {e:?}"))
    }
}

/// The tags of an XML text, in order (text content, comments and declarations skipped).
fn tags(text: &str) -> Vec<Tag> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        let after = &rest[start + 1..];
        let end = after.find('>').expect("unterminated tag");
        let inner = after[..end].trim();
        rest = &after[end + 1..];
        if inner.starts_with('?') || inner.starts_with('!') {
            continue;
        }
        let close = inner.starts_with('/');
        let inner = inner.trim_start_matches('/').trim_end_matches('/').trim();
        let name_end = inner.find(char::is_whitespace).unwrap_or(inner.len());
        out.push(Tag { name: inner[..name_end].to_string(), close, attrs: attributes(&inner[name_end..]) });
    }
    out
}

fn attributes(mut s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    while let Some(eq) = s.find('=') {
        let key = s[..eq].trim().to_string();
        let after = s[eq + 1..].trim_start();
        let Some(value) = after.strip_prefix('"') else { break };
        let end = value.find('"').expect("unterminated attribute");
        out.push((key, unescape(&value[..end])));
        s = &value[end + 1..];
    }
    out
}

fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("scriptoria-testkit-{}-{name}", std::process::id()))
    }

    #[test]
    fn strokes_round_trip() {
        let mut outline = BezPath::new();
        outline.move_to((0.1234567890123, 1.0));
        outline.curve_to((1.0, 2.0), (2.0, 2.0), (3.0, 1.0 / 3.0));
        outline.close_path();
        let mut line = BezPath::new();
        line.move_to((0.0, 0.0));
        line.line_to((1.0, 1.0));
        let groups = vec![
            Group {
                id: Some("佳".into()),
                note: Some("a \"note\" <with> & marks".into()),
                items: vec![Stroke { id: "a".into(), tags: vec!["hook".into(), "tip".into()], source: Some("page 2 #5".into()), kind: StrokeKind::Outline, path: outline }],
            },
            Group { id: None, note: None, items: vec![Stroke { id: "b".into(), tags: vec![], source: None, kind: StrokeKind::Line { width: 0.25 }, path: line }] },
        ];
        let file = temp("strokes.svg");
        write_strokes(&file, "round trip", &groups);
        let back = read_strokes(&file);
        let _ = std::fs::remove_file(&file);
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].id, groups[0].id);
        assert_eq!(back[0].note, groups[0].note);
        assert_eq!(back[0].items, groups[0].items);
        assert_eq!(back[1].items, groups[1].items);
    }

    #[test]
    fn skeletons_round_trip() {
        let (a, b, c) = (Point::new(0.0, 0.0), Point::new(1.0, 0.5), Point::new(2.0, 0.0));
        let piece = SkeletonPiece {
            circles: vec![(a, 0.1), (b, 0.15), (c, 1.0 / 7.0)],
            spine: vec![
                CubicBez::new(a, Point::new(0.3, 0.1), Point::new(0.7, 0.4), b),
                CubicBez::new(b, Point::new(1.3, 0.6), Point::new(1.7, 0.2), c),
            ],
        };
        let dot = SkeletonPiece { circles: vec![(Point::new(5.0, 5.0), 0.2)], spine: vec![] };
        let groups = vec![Group {
            id: Some("x".into()),
            note: None,
            items: vec![Skeleton { id: "s".into(), tags: vec!["hook".into()], source: None, pieces: vec![piece, dot] }],
        }];
        let file = temp("skeletons.svg");
        write_skeletons(&file, "round trip", &groups);
        let back = read_skeletons(&file);
        let _ = std::fs::remove_file(&file);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].items, groups[0].items);
    }
}
