//! Character sets a font is made for: the Unicode blocks (named in Chinese where handwriting is
//! mostly made of them), and national standards' sets of characters, each in its own order.
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
    vec![
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
    ]
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
        let tongyong: Vec<char> = sets[..3].iter().flat_map(|s| s.chars.clone()).collect();
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
