//! Character sets a font is made for: the Unicode blocks (named in Chinese where handwriting is
//! mostly made of them), and national standards' sets of characters, each in its own order: first
//! GB 18030-2022 (the mandatory Chinese character set), by its three implementation levels.
//!
//! The standards' sets come from their encodings' tables (as the `encoding_rs` crate has them,
//! after the WHATWG Encoding Standard), read over the code ranges each standard gives its levels:
//! GB 2312 (一级、二级汉字), Big5 (常用字、次常用字), JIS X 0208 (第一水準、第二水準) and KS X 1001
//! (한글、漢字). The 通用规范汉字表 (2013) has no encoding: its 8105 characters are kept here, in
//! the table's order (see `data/NOTICE`).

use std::ops::RangeInclusive;

use encoding_rs::{BIG5, EUC_JP, EUC_KR, Encoding, GBK};
use unicode_blocks::{UnicodeBlock, find_unicode_block};

/// A set of characters, in its own order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Charset {
    pub name: String,
    pub chars: Vec<char>,
}

/// A Unicode block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub start: u32,
    pub end: u32,
    /// Its name in the Unicode standard.
    pub english: &'static str,
}

impl From<UnicodeBlock> for Block {
    fn from(b: UnicodeBlock) -> Self {
        Block { start: b.start(), end: b.end(), english: b.name() }
    }
}

impl Block {
    /// Its name, in Chinese for the blocks handwriting is mostly made of.
    pub fn name(&self) -> String {
        if let Some(ext) = self.english.strip_prefix("CJK Unified Ideographs Extension ") {
            return format!("中日韩统一表意文字扩展 {ext}");
        }
        let chinese = match self.english {
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
            "Hangul Jamo" => "谚文字母",
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
            other => other,
        };
        chinese.to_string()
    }

    pub fn contains(&self, c: char) -> bool {
        (self.start..=self.end).contains(&(c as u32))
    }

    /// Its code points that are characters (no surrogates), in order.
    pub fn chars(&self) -> impl Iterator<Item = char> + use<> {
        (self.start..=self.end).filter_map(char::from_u32)
    }
}

/// The block a character is in.
pub fn block_of(c: char) -> Option<Block> {
    find_unicode_block(c).map(Block::from)
}

/// The block starting at a code point.
pub fn block(start: u32) -> Option<Block> {
    char::from_u32(start).and_then(block_of).filter(|b| b.start == start)
}

/// Every Unicode block, in code point order.
pub fn blocks() -> Vec<Block> {
    let mut out = Vec::new();
    let mut code = 0u32;
    while code <= char::MAX as u32 {
        match char::from_u32(code).and_then(block_of) {
            Some(b) => {
                out.push(b);
                code = b.end + 1;
            }
            None => code += 1,
        }
    }
    out
}

/// The 通用规范汉字表, in its order.
const TONGYONG: &str = include_str!("../data/tongyong-guifan.txt");

/// The characters at the double-byte codes of an encoding with a lead byte in `leads` and a trail
/// byte in `trails` (and the code, lead × 256 + trail, in `codes`), in code order; codes the table
/// leaves unassigned (or gives a private use character) left out.
fn table(encoding: &'static Encoding, leads: RangeInclusive<u8>, trails: &[RangeInclusive<u8>], codes: RangeInclusive<u16>) -> Vec<char> {
    let mut out = Vec::new();
    for lead in leads {
        for trail in trails.iter().flat_map(|t| t.clone()) {
            if !codes.contains(&(u16::from(lead) << 8 | u16::from(trail))) {
                continue;
            }
            let bytes = [lead, trail];
            let Some(text) = encoding.decode_without_bom_handling_and_without_replacement(&bytes) else { continue };
            let mut chars = text.chars();
            if let (Some(c), None) = (chars.next(), chars.next())
                && !('\u{E000}'..='\u{F8FF}').contains(&c)
            {
                out.push(c);
            }
        }
    }
    out
}

/// GB 18030-2022's unified ideographs, by the extents of Unicode 11 (the standard's level 1 leaves
/// out the last 16 of the basic block and the last 10 of extension A as later versions have them):
/// the basic block and extension A (level 1), extensions B to F (level 3).
const GB18030_BASIC: RangeInclusive<u32> = 0x4E00..=0x9FEF;
const GB18030_EXTENSION_A: RangeInclusive<u32> = 0x3400..=0x4DB5;
const GB18030_EXTENSIONS_B_TO_F: [RangeInclusive<u32>; 5] = [0x20000..=0x2A6D6, 0x2A700..=0x2B734, 0x2B740..=0x2B81D, 0x2B820..=0x2CEA1, 0x2CEB0..=0x2EBE0];
/// The unified ideographs among the compatibility ideographs (in GB 18030's two-byte part).
const UNIFIED_COMPATIBILITY: [u32; 12] = [0xFA0E, 0xFA0F, 0xFA11, 0xFA13, 0xFA14, 0xFA1F, 0xFA21, 0xFA23, 0xFA24, 0xFA27, 0xFA28, 0xFA29];
/// The Kangxi radicals (level 3).
const KANGXI_RADICALS: RangeInclusive<u32> = 0x2F00..=0x2FD5;

fn chars_in(range: RangeInclusive<u32>) -> impl Iterator<Item = char> {
    range.filter_map(char::from_u32)
}

/// GB 18030-2022's sets, cumulative as its implementation levels are: the Chinese characters of
/// level 1 (basic block, extension A, the 12 unified compatibility ideographs: 27,570), of level 2
/// (and the 196 of the 通用规范汉字表 in extensions B to E), of level 3 (all 87,887, and the 214
/// Kangxi radicals); and the other characters of its two-byte part (symbols, full-width forms,
/// kana, Greek, Cyrillic and the like).
fn gb18030(tongyong: &[char]) -> Vec<(&'static str, Vec<char>)> {
    let level1: Vec<char> = chars_in(GB18030_EXTENSION_A)
        .chain(chars_in(GB18030_BASIC))
        .chain(UNIFIED_COMPATIBILITY.iter().filter_map(|&c| char::from_u32(c)))
        .collect();
    let in_b_to_f = |c: char| GB18030_EXTENSIONS_B_TO_F.iter().any(|r| r.contains(&(c as u32)));
    let mut level2 = level1.clone();
    level2.extend(tongyong.iter().copied().filter(|&c| in_b_to_f(c)));
    let mut level3 = level1.clone();
    level3.extend(GB18030_EXTENSIONS_B_TO_F.iter().cloned().flat_map(chars_in));
    level3.extend(chars_in(KANGXI_RADICALS));
    let hanzi: std::collections::HashSet<char> = level3.iter().copied().collect();
    let two_byte: Vec<char> = table(encoding_rs::GB18030, 0x81..=0xFE, &[0x40..=0x7E, 0x80..=0xFE], 0..=u16::MAX)
        .into_iter()
        .filter(|c| !hanzi.contains(c))
        .collect();
    vec![
        ("GB 18030-2022 实现级别 1 汉字", level1),
        ("GB 18030-2022 实现级别 2 汉字", level2),
        ("GB 18030-2022 实现级别 3 汉字与康熙部首", level3),
        ("GB 18030 双字节部分的其他字符（符号、字母、假名等）", two_byte),
    ]
}

/// Punctuation Chinese text is written with.
const CHINESE_PUNCTUATION: &str = "，。、；：？！“”‘’（）《》〈〉【】〔〕「」『』—…·～";

/// The standards' sets, in the order listed.
pub fn standards() -> Vec<Charset> {
    let set = |name: &str, chars: Vec<char>| Charset { name: format!("{name}（{}）", chars.len()), chars };
    let tongyong: Vec<char> = TONGYONG.lines().filter_map(|l| l.trim().chars().next()).collect();
    let row = |a: u8, b: u8| a..=b;
    let full = [row(0xA1, 0xFE)];
    let big5 = [row(0x40, 0x7E), row(0xA1, 0xFE)];
    let any = 0..=u16::MAX;
    let mut sets: Vec<Charset> = gb18030(&tongyong).into_iter().map(|(name, chars)| set(name, chars)).collect();
    sets.extend([
        set("通用规范汉字表 一级字表", tongyong[..3500].to_vec()),
        set("通用规范汉字表 二级字表", tongyong[3500..6500].to_vec()),
        set("通用规范汉字表 三级字表", tongyong[6500..].to_vec()),
        set("GB 2312 一级汉字", table(GBK, row(0xB0, 0xD7), &full, any.clone())),
        set("GB 2312 二级汉字", table(GBK, row(0xD8, 0xF7), &full, any.clone())),
        set("Big5 常用字", table(BIG5, row(0xA4, 0xC6), &big5, 0xA440..=0xC67E)),
        set("Big5 次常用字", table(BIG5, row(0xC9, 0xF9), &big5, 0xC940..=0xF9D5)),
        set("JIS X 0208 第一水準漢字", table(EUC_JP, row(0xB0, 0xCF), &full, any.clone())),
        set("JIS X 0208 第二水準漢字", table(EUC_JP, row(0xD0, 0xF4), &full, any.clone())),
        set("KS X 1001 한글", table(EUC_KR, row(0xB0, 0xC8), &full, any.clone())),
        set("KS X 1001 漢字", table(EUC_KR, row(0xCA, 0xFD), &full, any)),
        set("ASCII 可打印字符", ('!'..='~').collect()),
        set("中文常用标点", CHINESE_PUNCTUATION.chars().collect()),
    ]);
    sets
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn find<'a>(sets: &'a [Charset], prefix: &str) -> &'a Charset {
        sets.iter().find(|s| s.name.starts_with(prefix)).unwrap_or_else(|| panic!("no {prefix}"))
    }

    #[test]
    fn the_standards_have_the_sizes_they_say() {
        let sets = standards();
        for (prefix, n) in [
            ("GB 18030-2022 实现级别 1", 27_570),
            ("GB 18030-2022 实现级别 2", 27_570 + 196),
            ("GB 18030-2022 实现级别 3", 87_887 + 214),
            ("通用规范汉字表 一级", 3500),
            ("通用规范汉字表 二级", 3000),
            ("通用规范汉字表 三级", 1605),
            ("GB 2312 一级", 3755),
            ("GB 2312 二级", 3008),
            ("Big5 常用", 5401),
            ("Big5 次常用", 7652),
            ("JIS X 0208 第一", 2965),
            ("JIS X 0208 第二", 3390),
            ("KS X 1001 한글", 2350),
            ("KS X 1001 漢字", 4888),
            ("ASCII", 94),
        ] {
            let set = find(&sets, prefix);
            assert_eq!(set.chars.len(), n, "{}", set.name);
            assert_eq!(set.chars.iter().collect::<HashSet<_>>().len(), n, "{} has no character twice", set.name);
        }
    }

    #[test]
    fn the_standards_start_and_end_where_they_should() {
        let sets = standards();
        let ends = |prefix: &str| {
            let s = &find(&sets, prefix).chars;
            (s[0], s[s.len() - 1])
        };
        // The 通用规范汉字表 by strokes: its levels' first and last characters.
        assert_eq!(ends("通用规范汉字表 一级"), ('一', '矗'));
        assert_eq!(ends("通用规范汉字表 二级"), ('乂', '齉'));
        assert_eq!(ends("通用规范汉字表 三级"), ('亍', '蠼'));
        // The encodings' levels: by pinyin, by radical, by strokes, by on'yomi, by reading.
        assert_eq!(ends("GB 2312 一级"), ('啊', '座'));
        assert_eq!(ends("GB 2312 二级"), ('亍', '齄'));
        assert_eq!(ends("Big5 常用"), ('一', '籲'));
        assert_eq!(ends("JIS X 0208 第一"), ('亜', '腕'));
        assert_eq!(ends("KS X 1001 한글"), ('가', '힝'));
        // Every level of the 通用规范汉字表 is unified ideographs (with its extensions).
        let tongyong: Vec<char> = sets.iter().filter(|s| s.name.starts_with("通用规范汉字表")).flat_map(|s| s.chars.clone()).collect();
        assert_eq!(tongyong.iter().collect::<HashSet<_>>().len(), 8105);
        assert!(tongyong.iter().all(|&c| block_of(c).is_some_and(|b| b.english.starts_with("CJK Unified Ideographs"))));
    }

    #[test]
    fn blocks_are_listed_and_named() {
        let all = blocks();
        assert!(all.len() > 300, "{}", all.len());
        assert!(all.windows(2).all(|w| w[0].end < w[1].start), "in order, apart");
        assert_eq!(all[0].name(), "基本拉丁字母");
        let hiragana = block(0x3040).unwrap();
        assert_eq!((hiragana.name(), hiragana.end), ("平假名".to_string(), 0x309F));
        assert!(hiragana.contains('あ') && !hiragana.contains('ア'));
        assert_eq!(block(0x20000).unwrap().name(), "中日韩统一表意文字扩展 B");
        assert_eq!(block(0x3041), None, "not where a block starts");
        assert_eq!(block_of('A').map(|b| b.start), Some(0));
    }
}

#[cfg(test)]
mod gb18030 {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn the_levels_hold_one_another() {
        let sets = standards();
        let set = |prefix: &str| -> HashSet<char> { sets.iter().find(|s| s.name.starts_with(prefix)).unwrap().chars.iter().copied().collect() };
        let (one, two, three) = (set("GB 18030-2022 实现级别 1"), set("GB 18030-2022 实现级别 2"), set("GB 18030-2022 实现级别 3"));
        assert!(one.is_subset(&two) && two.is_subset(&three));
        // Level 1 ends where GB 18030-2005's required part did; level 2 adds the 通用规范汉字表's
        // characters beyond it; level 3 is all 87,887 Chinese characters and the Kangxi radicals.
        assert!(one.contains(&'\u{9FEF}') && !one.contains(&'\u{9FF0}') && one.contains(&'\u{4DB5}') && !one.contains(&'\u{4DB6}'));
        let tongyong = set("通用规范汉字表 一级").into_iter().chain(set("通用规范汉字表 二级")).chain(set("通用规范汉字表 三级"));
        assert!(tongyong.into_iter().all(|c| two.contains(&c)), "level 2 has the whole 通用规范汉字表");
        assert!(three.contains(&'⼀') && three.contains(&'\u{2EBE0}') && !three.contains(&'\u{2EBE1}'));
        let hanzi = three.iter().filter(|&&c| !(0x2F00..=0x2FD5).contains(&(c as u32))).count();
        assert_eq!(hanzi, 87_887);
        // The two-byte part's other characters: punctuation, full-width forms, kana and the like.
        let other = set("GB 18030 双字节部分的其他字符");
        assert!(other.contains(&'，') && other.contains(&'Ａ') && other.contains(&'あ') && other.contains(&'α'));
        assert!(other.is_disjoint(&three));
    }
}
