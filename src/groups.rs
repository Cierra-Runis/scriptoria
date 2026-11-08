//! A project's glyphs sorted into groups, to work on many at once (as FontForge shows a font by
//! encoding and Unicode block): all of them; those written and those not yet; by Unicode block; by
//! the practice sheet they were given on. Also characters typed as ranges of code points.

use std::collections::{BTreeMap, HashMap, HashSet};

use scriptoria_project::Project;
use unicode_blocks::{UnicodeBlock, find_unicode_block};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    All,
    Unwritten,
    Written,
    /// A Unicode block, by its first code point.
    Block(u32),
    /// Glyphs whose character is in no block (or that have none).
    NoBlock,
    /// A practice sheet, by its id.
    Template(u32),
}

/// Where a group is listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Status,
    Blocks,
    Templates,
}

/// A group as listed: its name, how many glyphs it holds and how many of them are written.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    pub group: Group,
    pub section: Section,
    pub name: String,
    pub glyphs: usize,
    pub written: usize,
}

fn block_of(c: char) -> Option<UnicodeBlock> {
    find_unicode_block(c)
}

/// The block starting at a code point.
pub fn block(start: u32) -> Option<UnicodeBlock> {
    char::from_u32(start).and_then(block_of).filter(|b| b.start() == start)
}

/// A block's name, in Chinese for the blocks handwriting is mostly made of.
pub fn block_name(b: UnicodeBlock) -> String {
    let name = b.name();
    if let Some(ext) = name.strip_prefix("CJK Unified Ideographs Extension ") {
        return format!("中日韩统一表意文字扩展 {ext}");
    }
    let chinese = match name {
        "Basic Latin" => "基本拉丁字母",
        "Latin-1 Supplement" => "拉丁字母补充-1",
        "Latin Extended-A" => "拉丁字母扩展-A",
        "Latin Extended-B" => "拉丁字母扩展-B",
        "Latin Extended Additional" => "拉丁字母扩展附加",
        "IPA Extensions" => "国际音标扩展",
        "Greek and Coptic" => "希腊字母和科普特字母",
        "Cyrillic" => "西里尔字母",
        "General Punctuation" => "常用标点",
        "Superscripts and Subscripts" => "上标和下标",
        "Currency Symbols" => "货币符号",
        "Letterlike Symbols" => "类字母符号",
        "Number Forms" => "数字形式",
        "Arrows" => "箭头",
        "Mathematical Operators" => "数学运算符",
        "Enclosed Alphanumerics" => "带圈字母数字",
        "Box Drawing" => "制表符",
        "Geometric Shapes" => "几何图形",
        "Miscellaneous Symbols" => "杂项符号",
        "CJK Radicals Supplement" => "中日韩部首补充",
        "Kangxi Radicals" => "康熙部首",
        "CJK Symbols and Punctuation" => "中日韩符号和标点",
        "Hiragana" => "平假名",
        "Katakana" => "片假名",
        "Bopomofo" => "注音符号",
        "Hangul Compatibility Jamo" => "谚文兼容字母",
        "Kanbun" => "汉文训读",
        "Bopomofo Extended" => "注音符号扩展",
        "CJK Strokes" => "中日韩笔画",
        "Katakana Phonetic Extensions" => "片假名音标扩展",
        "Enclosed CJK Letters and Months" => "带圈中日韩字母和月份",
        "CJK Compatibility" => "中日韩兼容",
        "CJK Unified Ideographs" => "中日韩统一表意文字",
        "Hangul Syllables" => "谚文音节",
        "CJK Compatibility Ideographs" => "中日韩兼容表意文字",
        "Vertical Forms" => "竖排形式",
        "CJK Compatibility Forms" => "中日韩兼容形式",
        "Small Form Variants" => "小写变体形式",
        "Halfwidth and Fullwidth Forms" => "半角及全角形式",
        _ => name,
    };
    chinese.to_string()
}

/// The groups of a project, in the order listed: all, by state, by block (in code point order),
/// by practice sheet.
pub fn list(project: &Project) -> Vec<Listed> {
    let glyphs = &project.glyphs;
    let written = glyphs.iter().filter(|g| !g.strokes.is_empty()).count();
    let mut out = vec![
        Listed { group: Group::All, section: Section::Status, name: "全部".into(), glyphs: glyphs.len(), written },
        Listed { group: Group::Unwritten, section: Section::Status, name: "未书写".into(), glyphs: glyphs.len() - written, written: 0 },
        Listed { group: Group::Written, section: Section::Status, name: "已书写".into(), glyphs: written, written },
    ];
    let mut blocks: BTreeMap<u32, (UnicodeBlock, usize, usize)> = BTreeMap::new();
    let mut none = (0, 0);
    for g in glyphs {
        let w = usize::from(!g.strokes.is_empty());
        match g.chars.first().copied().and_then(block_of) {
            Some(b) => {
                let e = blocks.entry(b.start()).or_insert((b, 0, 0));
                e.1 += 1;
                e.2 += w;
            }
            None => {
                none.0 += 1;
                none.1 += w;
            }
        }
    }
    out.extend(blocks.into_values().map(|(b, n, w)| Listed { group: Group::Block(b.start()), section: Section::Blocks, name: block_name(b), glyphs: n, written: w }));
    if none.0 > 0 {
        out.push(Listed { group: Group::NoBlock, section: Section::Blocks, name: "其他".into(), glyphs: none.0, written: none.1 });
    }
    let by_char = chars_to_glyphs(project);
    for t in &project.templates {
        let members: Vec<usize> = template_members(&by_char, &t.chars);
        let w = members.iter().filter(|&&g| !glyphs[g].strokes.is_empty()).count();
        out.push(Listed { group: Group::Template(t.id), section: Section::Templates, name: format!("模板 #{}", t.id), glyphs: members.len(), written: w });
    }
    out
}

fn chars_to_glyphs(project: &Project) -> HashMap<char, usize> {
    let mut map = HashMap::new();
    for (i, g) in project.glyphs.iter().enumerate() {
        for &c in &g.chars {
            map.entry(c).or_insert(i);
        }
    }
    map
}

/// A sheet's glyphs in the sheet's order (each once).
fn template_members(by_char: &HashMap<char, usize>, chars: &str) -> Vec<usize> {
    let mut seen = HashSet::new();
    chars.chars().filter_map(|c| by_char.get(&c).copied()).filter(|g| seen.insert(*g)).collect()
}

/// The glyphs of a group: in the project's order, or a sheet's in the sheet's.
pub fn members(project: &Project, group: Group) -> Vec<usize> {
    let glyphs = &project.glyphs;
    let first_block = |i: usize| glyphs[i].chars.first().copied().and_then(block_of).map(|b| b.start());
    let all = 0..glyphs.len();
    match group {
        Group::All => all.collect(),
        Group::Unwritten => all.filter(|&i| glyphs[i].strokes.is_empty()).collect(),
        Group::Written => all.filter(|&i| !glyphs[i].strokes.is_empty()).collect(),
        Group::Block(start) => all.filter(|&i| first_block(i) == Some(start)).collect(),
        Group::NoBlock => all.filter(|&i| first_block(i).is_none()).collect(),
        Group::Template(id) => project.templates.iter().find(|t| t.id == id).map(|t| template_members(&chars_to_glyphs(project), &t.chars)).unwrap_or_default(),
    }
}

/// The characters of a block the project has no glyph for, among those `has` (say, those the
/// reference font draws).
pub fn missing_in_block(project: &Project, start: u32, has: impl Fn(char) -> bool) -> String {
    let Some(b) = block(start) else { return String::new() };
    let have: HashSet<char> = project.glyphs.iter().flat_map(|g| g.chars.iter().copied()).collect();
    (b.start()..=b.end()).filter_map(char::from_u32).filter(|c| !have.contains(c) && has(*c)).collect()
}

/// The most characters one range typed may give.
pub const RANGE_LIMIT: u32 = 0x10000;

/// Text typed to add characters, with code points and ranges of them (`U+4E00`, `U+3041-U+3096`,
/// `U+3041..3096`) written out as the characters; the rest as typed.
pub fn expand_ranges(text: &str) -> String {
    text.split_whitespace().map(|word| range(word).unwrap_or_else(|| word.to_string())).collect::<Vec<_>>().join(" ")
}

fn range(word: &str) -> Option<String> {
    let code = |s: &str| {
        let s = s.strip_prefix("U+").or_else(|| s.strip_prefix("u+")).unwrap_or(s);
        (4..=6).contains(&s.len()).then(|| u32::from_str_radix(s, 16).ok()).flatten()
    };
    let start_text = word.strip_prefix("U+").or_else(|| word.strip_prefix("u+"))?;
    let (from, to) = match start_text.split_once("..").or_else(|| start_text.split_once('-')).or_else(|| start_text.split_once('–')) {
        Some((a, b)) => (code(a)?, code(b)?),
        None => {
            let c = code(start_text)?;
            (c, c)
        }
    };
    (from <= to && to - from < RANGE_LIMIT).then(|| (from..=to).filter_map(char::from_u32).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use scriptoria_ink::kurbo::{Affine, BezPath};
    use scriptoria_project::{GlyphStroke, Outline, OutlineKind};

    fn written(project: &mut Project, c: char) {
        let g = project.glyph_for(c).unwrap();
        project.glyphs[g].strokes.push(GlyphStroke {
            origin: None,
            outline: Outline { path: BezPath::new(), kind: OutlineKind::Filled },
            skeleton: Vec::new(),
            placement: Affine::IDENTITY,
            pen: 1.0,
        });
    }

    #[test]
    fn glyphs_group_by_state_block_and_sheet() {
        let mut p = Project::new("x");
        p.add_chars("世界あいAb");
        written(&mut p, '世');
        written(&mut p, 'A');
        p.new_template(&['い', '界', 'Z']);
        let listed = list(&p);
        let find = |g: Group| listed.iter().find(|l| l.group == g).map(|l| (l.name.as_str(), l.glyphs, l.written));
        assert_eq!(find(Group::All), Some(("全部", 6, 2)));
        assert_eq!(find(Group::Unwritten), Some(("未书写", 4, 0)));
        assert_eq!(find(Group::Block(0x4E00)), Some(("中日韩统一表意文字", 2, 1)));
        assert_eq!(find(Group::Block(0x3040)), Some(("平假名", 2, 0)));
        assert_eq!(find(Group::Block(0)), Some(("基本拉丁字母", 2, 1)));
        // The sheet's characters with glyphs (Z has none).
        assert_eq!(find(Group::Template(1)), Some(("模板 #1", 2, 0)));
        // Blocks in code point order.
        let blocks: Vec<Group> = listed.iter().filter(|l| l.section == Section::Blocks).map(|l| l.group).collect();
        assert_eq!(blocks, [Group::Block(0), Group::Block(0x3040), Group::Block(0x4E00)]);

        assert_eq!(members(&p, Group::Block(0x4E00)), [0, 1]);
        assert_eq!(members(&p, Group::Written), [0, 4]);
        // A sheet's glyphs in the sheet's order.
        assert_eq!(members(&p, Group::Template(1)), [3, 1]);
    }

    #[test]
    fn blocks_are_filled_in() {
        let mut p = Project::new("x");
        p.add_chars("あい");
        let missing = missing_in_block(&p, 0x3040, |c| ('ぁ'..='う').contains(&c));
        assert_eq!(missing, "ぁぃぅう");
        assert_eq!(block(0x3041), None, "not where a block starts");
        assert_eq!(block_name(block(0x20000).unwrap()), "中日韩统一表意文字扩展 B");
    }

    #[test]
    fn ranges_are_written_out() {
        assert_eq!(expand_ranges("U+3041-U+3045"), "ぁあぃいぅ");
        assert_eq!(expand_ranges("u+4E00..4E02 天 U+0041"), "一丁丂 天 A");
        // Not ranges: as typed.
        assert_eq!(expand_ranges("U+12 abc U+3045-U+3041"), "U+12 abc U+3045-U+3041");
        assert_eq!(expand_ranges("U+0000-U+FFFFF"), "U+0000-U+FFFFF");
    }
}
