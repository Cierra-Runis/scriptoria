//! What Chinese characters are made of, from ideographic description sequences (IDS): 清 is 氵
//! beside 青 (⿰氵青). A [`Table`] answers which characters have a component, where it sits in them
//! (a box of the em, by the layout the sequences give: ⿰ halves left and right, and so on), and
//! which components a character can be put together from.
//!
//! [`Table::builtin`] holds BabelStone's IDS data (the G source forms; see `data/NOTICE`).

use std::collections::HashMap;
use std::sync::OnceLock;

use kurbo::Rect;

/// A character's makeup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ids {
    /// A component that is a character (or a stroke, or a radical form).
    Char(char),
    /// A component with no character: unencoded (`{n}`), or not representable (`？`).
    Unknown,
    /// Components laid out by an ideographic description character (`⿰` and the others), or put
    /// together otherwise (`⿻` overlaid, `㇯` the second taken from the first, `⿾` mirrored, `⿿`
    /// turned).
    Op(char, Vec<Ids>),
}

/// How many components an operator takes.
fn arity(op: char) -> Option<usize> {
    match op {
        '⿰' | '⿱' | '⿴' | '⿵' | '⿶' | '⿷' | '⿸' | '⿹' | '⿺' | '⿻' | '⿼' | '⿽' | '㇯' => Some(2),
        '⿲' | '⿳' => Some(3),
        '⿾' | '⿿' => Some(1),
        _ => None,
    }
}

/// Whether an operator puts one component inside another (whose writing order is often not the
/// sequence's: 国's last stroke closes it, 这's 辶 comes after what it carries).
pub fn encloses(op: char) -> bool {
    matches!(op, '⿴' | '⿵' | '⿶' | '⿷' | '⿸' | '⿹' | '⿺' | '⿼' | '⿽')
}

impl Ids {
    /// Reads a sequence (a `〾` before it is passed over); none if it is not one.
    pub fn parse(text: &str) -> Option<Ids> {
        let mut chars = text.trim().trim_start_matches('〾').chars().peekable();
        let ids = Self::read(&mut chars, 0)?;
        chars.next().is_none().then_some(ids)
    }

    fn read(chars: &mut std::iter::Peekable<std::str::Chars>, depth: usize) -> Option<Ids> {
        if depth > 64 {
            return None;
        }
        let c = chars.next()?;
        if c == '{' {
            // An unencoded component: {n}.
            for d in chars.by_ref() {
                if d == '}' {
                    return Some(Ids::Unknown);
                }
            }
            return None;
        }
        if c == '？' {
            return Some(Ids::Unknown);
        }
        match arity(c) {
            Some(n) => {
                let parts = (0..n).map(|_| Self::read(chars, depth + 1)).collect::<Option<Vec<_>>>()?;
                Some(Ids::Op(c, parts))
            }
            None => Some(Ids::Char(c)),
        }
    }

    /// Where each of its components sits, when it fills `area`.
    pub fn regions(&self, area: Rect) -> Vec<Rect> {
        let Ids::Op(op, parts) = self else { return Vec::new() };
        let (x0, y0, w, h) = (area.x0, area.y0, area.width(), area.height());
        // A box of `area` by fractions (x from the left, y from the bottom).
        let at = |a: f64, b: f64, c: f64, d: f64| Rect::new(x0 + a * w, y0 + b * h, x0 + c * w, y0 + d * h);
        let inner = match op {
            '⿰' => return vec![at(0.0, 0.0, 0.5, 1.0), at(0.5, 0.0, 1.0, 1.0)],
            '⿱' => return vec![at(0.0, 0.5, 1.0, 1.0), at(0.0, 0.0, 1.0, 0.5)],
            '⿲' => return (0..3).map(|i| at(i as f64 / 3.0, 0.0, (i + 1) as f64 / 3.0, 1.0)).collect(),
            '⿳' => return (0..3).map(|i| at(0.0, 1.0 - (i + 1) as f64 / 3.0, 1.0, 1.0 - i as f64 / 3.0)).collect(),
            '⿴' => at(0.2, 0.2, 0.8, 0.8),
            '⿵' => at(0.2, 0.0, 0.8, 0.75),
            '⿶' => at(0.2, 0.25, 0.8, 1.0),
            '⿷' => at(0.25, 0.2, 1.0, 0.8),
            '⿸' => at(0.3, 0.0, 1.0, 0.7),
            '⿹' => at(0.0, 0.0, 0.7, 0.7),
            '⿺' => at(0.3, 0.3, 1.0, 1.0),
            '⿼' => at(0.0, 0.2, 0.75, 0.8),
            '⿽' => at(0.0, 0.3, 0.7, 1.0),
            // Overlaid, taken away, mirrored, turned: each where the whole is.
            _ => return vec![area; parts.len()],
        };
        vec![area, inner]
    }
}

/// Characters' decompositions.
pub struct Table {
    sequences: HashMap<char, &'static str>,
}

/// Radical forms the data uses and the characters people name them by, both ways: a part named
/// 月 is the ⺝ of 青.
const VARIANTS: [(char, char); 12] = [
    ('⺝', '月'),
    ('⺼', '月'),
    ('⺡', '氵'),
    ('⺅', '亻'),
    ('⺘', '扌'),
    ('⺖', '忄'),
    ('⺮', '竹'),
    ('⺾', '艹'),
    ('⻌', '辶'),
    ('⻊', '足'),
    ('⻖', '阝'),
    ('⻏', '阝'),
];

/// Whether two components are the same, as far as their forms go.
pub fn same_component(a: char, b: char) -> bool {
    a == b || VARIANTS.iter().any(|&(x, y)| (x == a && y == b) || (x == b && y == a))
}

/// How deep components are looked into (a component of a component of …).
const DEPTH: usize = 12;

impl Table {
    /// Reads lines of "character<TAB>sequence" (lines starting with `#` are passed over).
    pub fn parse(text: &'static str) -> Table {
        let sequences = text
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| {
                let (c, ids) = l.split_once('\t')?;
                let mut chars = c.chars();
                let c = chars.next().filter(|_| chars.next().is_none())?;
                Some((c, ids.trim_end()))
            })
            .collect();
        Table { sequences }
    }

    /// BabelStone's data (the G source forms), read once.
    pub fn builtin() -> &'static Table {
        static TABLE: OnceLock<Table> = OnceLock::new();
        TABLE.get_or_init(|| Table::parse(include_str!("../data/ids.txt")))
    }

    /// How many characters it decomposes.
    pub fn len(&self) -> usize {
        self.sequences.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sequences.is_empty()
    }

    /// A character's makeup, one level: none for a character not decomposed (口, or one not in
    /// the data).
    pub fn decompose(&self, c: char) -> Option<Ids> {
        Ids::parse(self.sequences.get(&c)?)
    }

    /// Where `component` is in `c` (once for each time it is: 品 has 口 three times), as boxes of
    /// the em in fractions (x from the left, y from the bottom), components of components looked
    /// into. `c` itself is its own component, filling the em.
    pub fn occurrences(&self, c: char, component: char) -> Vec<Rect> {
        let whole = Rect::new(0.0, 0.0, 1.0, 1.0);
        let mut found = Vec::new();
        let target = self.decompose(component);
        self.find(&Ids::Char(c), component, target.as_ref(), whole, DEPTH, &mut found);
        found
    }

    fn find(&self, ids: &Ids, component: char, target: Option<&Ids>, area: Rect, depth: usize, found: &mut Vec<Rect>) {
        if depth == 0 {
            return;
        }
        match ids {
            Ids::Char(c) if same_component(*c, component) => found.push(area),
            Ids::Char(c) => {
                if let Some(inner) = self.decompose(*c) {
                    self.find(&inner, component, target, area, depth - 1, found);
                }
            }
            Ids::Unknown => {}
            // The component written out (吕 as ⿱口口).
            Ids::Op(..) if Some(ids) == target => found.push(area),
            Ids::Op(op, parts) => {
                for (i, (part, region)) in parts.iter().zip(ids.regions(area)).enumerate() {
                    // What is taken away is not there.
                    if *op == '㇯' && i == 1 {
                        continue;
                    }
                    self.find(part, component, target, region, depth - 1, found);
                }
            }
        }
    }

    /// Whether `component` is in `c`.
    pub fn contains(&self, c: char, component: char) -> bool {
        c != component && !self.occurrences(c, component).is_empty()
    }

    /// `c` put together from components `have` has (looked into as deep as it takes), in the
    /// sequence's order. None if some piece is not there, or is overlaid, taken away, mirrored or
    /// turned. `c` itself is not one of its pieces.
    pub fn compose(&self, c: char, have: impl Fn(char) -> bool) -> Option<Composition> {
        let ids = self.decompose(c)?;
        let mut composition = Composition { pieces: Vec::new(), enclosed: false };
        self.cover(&ids, &have, Rect::new(0.0, 0.0, 1.0, 1.0), DEPTH, &mut composition).then_some(composition)
    }

    fn cover(&self, ids: &Ids, have: &impl Fn(char) -> bool, area: Rect, depth: usize, into: &mut Composition) -> bool {
        if depth == 0 {
            return false;
        }
        match ids {
            Ids::Char(c) if have(*c) => {
                into.pieces.push((*c, area));
                true
            }
            Ids::Char(c) => self.decompose(*c).is_some_and(|inner| self.cover(&inner, have, area, depth - 1, into)),
            Ids::Unknown => false,
            Ids::Op('⿻' | '㇯' | '⿾' | '⿿', _) => false,
            Ids::Op(op, parts) => {
                into.enclosed |= encloses(*op);
                parts.iter().zip(ids.regions(area)).all(|(p, r)| self.cover(p, have, r, depth - 1, into))
            }
        }
    }
}

/// A character put together from components.
#[derive(Clone, Debug, PartialEq)]
pub struct Composition {
    /// Each component, with its box of the em (fractions, x from the left, y from the bottom), in
    /// the sequence's order: left to right, top to bottom, outside before inside.
    pub pieces: Vec<(char, Rect)>,
    /// Whether one is put inside another (see [`encloses`]): then the writing order wants a look.
    pub enclosed: bool,
}
