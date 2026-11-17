//! Components found in characters, where they sit, and characters put together from them.

use kurbo::Rect;
use scriptoria_ids::{Ids, Table};

fn table() -> Table {
    Table::parse(include_str!("data/ids.txt"))
}

fn near(a: Rect, b: Rect) -> bool {
    [(a.x0, b.x0), (a.y0, b.y0), (a.x1, b.x1), (a.y1, b.y1)].iter().all(|(x, y)| (x - y).abs() < 1e-9)
}

#[test]
fn sequences_are_read() {
    let t = table();
    assert_eq!(t.decompose('清'), Some(Ids::Op('⿰', vec![Ids::Char('氵'), Ids::Char('青')])));
    assert_eq!(t.decompose('营'), Some(Ids::Op('⿱', vec![Ids::Op('⿱', vec![Ids::Char('艹'), Ids::Char('冖')]), Ids::Char('吕')])));
    // Not decomposed: a component of its own, or not in the data.
    assert_eq!(t.decompose('口'), None);
    // A variation mark before it is passed over.
    assert!(matches!(t.decompose('七'), Some(Ids::Op('⿻', _))));
    assert_eq!(Ids::parse("⿰{12}口"), Some(Ids::Op('⿰', vec![Ids::Unknown, Ids::Char('口')])));
    assert_eq!(Ids::parse("⿰口"), None);
}

#[test]
fn components_are_found_where_they_sit() {
    let t = table();
    // 氵 on the left of 清 and of 河, alike.
    let left = Rect::new(0.0, 0.0, 0.5, 1.0);
    assert!(near(t.occurrences('清', '氵')[0], left));
    assert!(near(t.occurrences('河', '氵')[0], left));
    assert!(t.contains('清', '青') && !t.contains('河', '青'));
    // 品: 口 on top, and twice below (through 吅).
    let mut kou = t.occurrences('品', '口');
    kou.sort_by(|a, b| a.x0.total_cmp(&b.x0).then(a.y0.total_cmp(&b.y0)));
    assert_eq!(kou.len(), 3);
    assert!(near(kou[0], Rect::new(0.0, 0.0, 0.5, 0.5)));
    assert!(near(kou[1], Rect::new(0.0, 0.5, 1.0, 1.0)));
    assert!(near(kou[2], Rect::new(0.5, 0.0, 1.0, 0.5)));
    // Inside 囗: 玉, and 囗 around it all.
    assert!(near(t.occurrences('国', '玉')[0], Rect::new(0.2, 0.2, 0.8, 0.8)));
    assert!(near(t.occurrences('国', '囗')[0], Rect::new(0.0, 0.0, 1.0, 1.0)));
    // Deeper: 寸 in 谢 (right of 射, right of 谢).
    assert!(near(t.occurrences('谢', '寸')[0], Rect::new(0.75, 0.0, 1.0, 1.0)));
    // A radical's form and the character it is named by: 月 is the ⺝ of 青.
    assert!(t.contains('清', '月'));
    // 吕 in 宫 and in 营 (the lower half of each).
    assert!(near(t.occurrences('宫', '吕')[0], Rect::new(0.0, 0.0, 1.0, 0.5)));
    assert!(near(t.occurrences('营', '吕')[0], Rect::new(0.0, 0.0, 1.0, 0.5)));
    // What is taken away is not there.
    assert!(!t.contains('𬻌', '㇒') && t.contains('𬻌', '東'));
}

#[test]
fn characters_are_put_together_from_components() {
    let t = table();
    let have = |set: &'static str| move |c: char| set.contains(c);
    // 清 = 氵 + 青, left and right.
    let qing = t.compose('清', have("氵青")).unwrap();
    assert_eq!(qing.pieces.iter().map(|p| p.0).collect::<String>(), "氵青");
    assert!(near(qing.pieces[1].1, Rect::new(0.5, 0.0, 1.0, 1.0)) && !qing.enclosed);
    // Looked into as deep as it takes: 品 from 口 alone, in the sequence's order.
    let pin = t.compose('品', have("口")).unwrap();
    assert_eq!(pin.pieces.len(), 3);
    // Without a piece, none.
    assert!(t.compose('清', have("氵")).is_none());
    // 国: one inside the other, to look at.
    assert!(t.compose('国', have("囗玉")).unwrap().enclosed);
    // Overlaid: not put together.
    assert!(t.compose('七', have("一乚")).is_none());
}

#[test]
fn the_builtin_data_is_there() {
    let t = Table::builtin();
    assert!(t.len() > 90_000);
    assert!(t.contains('清', '氵') && t.contains('湖', '氵'));
}
