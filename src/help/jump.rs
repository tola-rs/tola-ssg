use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use super::layout::LinkSpan;
use super::model::LinkTarget;
use crate::terminal::style::Palette;

const ALPHABET: &[u8; 9] = b"asdfghjkl";
const CAPACITY: usize = ALPHABET.len() * ALPHABET.len();

pub(super) struct Jump {
    labels: Vec<Label>,
    link_cells: Vec<LinkCells>,
    selection: Selection,
    overflow: bool,
}

struct Label {
    index: usize,
    fragments: Vec<LinkSpan>,
}

struct LinkCells {
    line: usize,
    columns: Range<usize>,
}

#[derive(Clone, Copy)]
enum Selection {
    Undrawn,
    Single,
    Pairs,
    Narrowed,
}

impl Jump {
    pub(super) fn new(links: &[LinkSpan], visible: Range<usize>, columns: usize) -> Option<Self> {
        let mut links = links
            .iter()
            .filter(|link| visible.contains(&link.line) && link.column < columns && link.width > 0)
            .collect::<Vec<_>>();
        let link_cells = links
            .iter()
            .map(|link| LinkCells {
                line: link.line,
                columns: link.column..link.column.saturating_add(link.width),
            })
            .collect();
        links.sort_by_key(|link| (link.line, link.column));
        if links.is_empty() {
            return None;
        }
        let mut occurrences = BTreeMap::<usize, usize>::new();
        let mut labels = Vec::<Label>::new();
        let mut overflow = false;
        for link in links {
            if let Some(index) = occurrences.get(&link.id) {
                labels[*index].fragments.push(link.clone());
            } else if labels.len() < CAPACITY {
                let index = labels.len();
                occurrences.insert(link.id, index);
                labels.push(Label {
                    index,
                    fragments: vec![link.clone()],
                });
            } else {
                overflow = true;
            }
        }
        Some(Self {
            labels,
            link_cells,
            selection: Selection::Undrawn,
            overflow,
        })
    }

    pub(super) fn key(&mut self, character: char) -> Option<LinkTarget> {
        match self.selection {
            Selection::Undrawn => None,
            Selection::Single | Selection::Narrowed => self
                .labels
                .iter()
                .find(|label| char::from(ALPHABET[label.index % ALPHABET.len()]) == character)
                .map(|label| label.fragments[0].target.clone()),
            Selection::Pairs => {
                if self
                    .labels
                    .iter()
                    .any(|label| char::from(ALPHABET[label.index / ALPHABET.len()]) == character)
                {
                    self.labels.retain(|label| {
                        char::from(ALPHABET[label.index / ALPHABET.len()]) == character
                    });
                    self.selection = Selection::Narrowed;
                }
                None
            }
        }
    }

    pub(super) fn caption(&self) -> String {
        if self.labels.is_empty() {
            return "no links on screen · Esc/Tab cancels".to_owned();
        }
        let mut caption = match self.selection {
            Selection::Narrowed => "press the second label key · Esc/Tab cancels",
            Selection::Pairs => "press a label pair · Esc/Tab cancels",
            Selection::Undrawn if self.labels.len() > ALPHABET.len() => {
                "press a label pair · Esc/Tab cancels"
            }
            _ => "press a label · Esc/Tab cancels",
        }
        .to_owned();
        if self.overflow {
            caption.push_str(" · the rest need scrolling");
        }
        caption
    }

    pub(super) fn draw(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        top: usize,
        palette: Palette,
    ) -> bool {
        loop {
            if !matches!(self.selection, Selection::Narrowed) {
                self.selection = if self.labels.len() <= ALPHABET.len() {
                    Selection::Single
                } else {
                    Selection::Pairs
                };
            }
            let width = if matches!(self.selection, Selection::Pairs) {
                2
            } else {
                1
            };
            let mut claimed = BTreeSet::new();
            let mut places = Vec::new();
            self.labels.retain(|label| {
                let Some(cells) = label.fragments.iter().find_map(|link| {
                    label_position(
                        frame.buffer_mut(),
                        area,
                        top,
                        link,
                        width,
                        &claimed,
                        &self.link_cells,
                    )
                }) else {
                    return false;
                };
                claimed.extend((cells.x..cells.right()).map(|column| (column, cells.y)));
                places.push(cells);
                true
            });
            // Pruning can shorten every label; assign the surviving keys before painting.
            if matches!(self.selection, Selection::Pairs) && self.labels.len() <= ALPHABET.len() {
                continue;
            }
            let mut placed = std::mem::take(&mut self.labels)
                .into_iter()
                .zip(places)
                .collect::<Vec<_>>();
            if !matches!(self.selection, Selection::Narrowed) {
                placed.sort_by_key(|(_, cells)| (cells.y, cells.x));
                for (index, (label, _)) in placed.iter_mut().enumerate() {
                    label.index = index;
                }
            }
            for (label, cells) in &placed {
                let keys = [
                    ALPHABET[label.index / ALPHABET.len()],
                    ALPHABET[label.index % ALPHABET.len()],
                ];
                let shown = if matches!(self.selection, Selection::Pairs) {
                    &keys[..]
                } else {
                    &keys[1..]
                };
                for column in cells.x..cells.right() {
                    frame.buffer_mut()[(column, cells.y)].set_symbol(" ");
                }
                for (offset, key) in shown.iter().enumerate() {
                    frame.buffer_mut()[(cells.x + offset as u16, cells.y)]
                        .set_symbol(&char::from(*key).to_string())
                        .set_style(palette.label_style());
                }
            }
            self.labels = placed.into_iter().map(|(label, _)| label).collect();
            break;
        }
        !self.labels.is_empty()
    }
}

fn label_position(
    buffer: &Buffer,
    area: Rect,
    top: usize,
    link: &LinkSpan,
    width: u16,
    claimed: &BTreeSet<(u16, u16)>,
    link_cells: &[LinkCells],
) -> Option<Rect> {
    let clipped = area.intersection(buffer.area);
    let offset = link.line.checked_sub(top)?;
    if offset >= usize::from(area.height) {
        return None;
    }
    let row = u16::try_from(usize::from(area.y).checked_add(offset)?).ok()?;
    let target = u16::try_from(usize::from(area.x).checked_add(link.column)?).ok()?;
    if !clipped.contains((target, row).into()) {
        return None;
    }
    let preferred = target.checked_sub(width);
    let link_end = usize::from(target).checked_add(link.width)?;
    let inside_link = |column: u16| usize::from(column) + usize::from(width) <= link_end;
    let fallback = target.checked_add(1).filter(|column| inside_link(*column));
    let first = Some(target).filter(|column| inside_link(*column));
    [preferred, fallback, first]
        .into_iter()
        .flatten()
        .find_map(|column| {
            let right = column.checked_add(width)?;
            if column < clipped.x || right > clipped.right() {
                return None;
            }
            if column > buffer.area.x && buffer[(column - 1, row)].symbol().width() > 1 {
                return None;
            }
            let end = (column..right).try_fold(right, |end, column| {
                let occupied = u16::try_from(buffer[(column, row)].symbol().width().max(1)).ok()?;
                Some(end.max(column.checked_add(occupied)?))
            })?;
            if end > clipped.right() || (column >= target && usize::from(end) > link_end) {
                return None;
            }
            if column < target
                && (column..end).any(|column| {
                    buffer[(column, row)].symbol() != " "
                        || link_cells.iter().any(|cells| {
                            cells.line == link.line
                                && cells.columns.contains(&usize::from(column - area.x))
                        })
                })
            {
                return None;
            }
            if (column..end).any(|column| claimed.contains(&(column, row))) {
                return None;
            }
            Some(Rect::new(column, row, end - column, 1))
        })
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::widgets::Paragraph;

    use super::*;

    fn links(count: usize) -> Vec<LinkSpan> {
        (0..count)
            .map(|id| LinkSpan {
                id,
                line: id,
                column: 3,
                width: 4,
                target: LinkTarget::External(format!("https://example.com/{id}")),
            })
            .collect()
    }

    fn draw(jump: &mut Jump, area: Rect, top: usize, text: &str) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(area.right(), area.bottom())).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(Paragraph::new(text), area);
                jump.draw(frame, area, top, Palette::new(false));
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    #[test]
    fn wrapped_occurrences_share_one_label() {
        let mut links = links(4);
        links[0].line = 3;
        links[1].id = links[0].id;
        links[1].line = 0;
        links[1].target = links[0].target.clone();
        links[2].id = links[0].id;
        links[2].line = 2;
        links[2].target = links[0].target.clone();
        links[3].line = 4;
        links[3].target = links[0].target.clone();
        let mut jump = Jump::new(&links, 1..5, 20).unwrap();
        draw(&mut jump, Rect::new(0, 0, 20, 4), 1, "");
        assert_eq!(jump.key('a'), Some(links[0].target.clone()));
        assert_eq!(jump.key('s'), Some(links[3].target.clone()));
        assert_eq!(jump.key('d'), None);
    }

    #[test]
    fn overflow_requires_scrolling() {
        let links = links(90);
        let mut jump = Jump::new(&links, 0..90, 20).unwrap();
        draw(&mut jump, Rect::new(0, 0, 20, 90), 0, "");
        assert!(jump.caption().contains("scrolling"));
        assert_eq!(jump.key('l'), None);
        assert_eq!(jump.key('l'), Some(links[80].target.clone()));
    }

    #[test]
    fn invisible_links_cannot_jump() {
        let links = links(2);
        let mut jump = Jump::new(&links, 0..2, 20).unwrap();
        assert_eq!(jump.key('a'), None);
        draw(&mut jump, Rect::new(5, 2, 10, 1), 1, "");
        assert_eq!(jump.key('a'), Some(links[1].target.clone()));
        assert_eq!(jump.key('s'), None);

        draw(&mut jump, Rect::new(5, 2, 0, 1), 1, "");
        assert_eq!(jump.key('a'), None);
    }

    #[test]
    fn clipped_labels_use_visible_keys() {
        let links = links(12);
        let mut jump = Jump::new(&links, 0..12, 20).unwrap();
        draw(&mut jump, Rect::new(5, 2, 20, 9), 3, "");
        assert_eq!(jump.key('a'), Some(links[3].target.clone()));
        assert_eq!(jump.key('l'), Some(links[11].target.clone()));
    }

    #[test]
    fn wrapped_labels_use_drawable_fragments() {
        let mut links = links(12);
        links[0].column = 19;
        links[0].width = 1;
        let continuation = LinkSpan {
            id: links[0].id,
            line: 12,
            column: 3,
            width: 4,
            target: links[0].target.clone(),
        };
        links.push(continuation);
        let mut jump = Jump::new(&links, 0..13, 20).unwrap();
        let buffer = draw(&mut jump, Rect::new(5, 2, 20, 13), 0, &"x".repeat(20));
        assert_eq!(buffer[(6, 14)].symbol(), "s");
        assert_eq!(buffer[(7, 14)].symbol(), "d");
        assert_eq!(jump.key('s'), None);
        assert_eq!(jump.key('d'), Some(links[0].target.clone()));
    }

    #[test]
    fn wide_links_remain_selectable() {
        let mut links = links(2);
        links[0].line = 4;
        links[0].column = 2;
        links[0].width = 2;
        links[1].line = 4;
        links[1].column = 4;
        links[1].width = 2;
        for columns in [5, 6] {
            let mut jump = Jump::new(&links, 4..5, 6).unwrap();
            let mut terminal = Terminal::new(TestBackend::new(12, 4)).unwrap();
            terminal
                .draw(|frame| {
                    frame.render_widget(Paragraph::new("界界界"), Rect::new(5, 2, 6, 1));
                    assert!(jump.draw(frame, Rect::new(5, 2, columns, 1), 4, Palette::new(false)));
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            assert_eq!(buffer[(5, 2)].symbol(), "界");
            assert_eq!(buffer[(7, 2)].symbol(), "a");
            assert_eq!(buffer[(8, 2)].symbol(), " ");
            assert_eq!(jump.key('a'), Some(links[0].target.clone()));
            if columns == 6 {
                assert_eq!(buffer[(9, 2)].symbol(), "s");
                assert_eq!(buffer[(10, 2)].symbol(), " ");
                assert_eq!(jump.key('s'), Some(links[1].target.clone()));
            } else {
                assert_eq!(buffer[(9, 2)].symbol(), "界");
                assert_eq!(jump.key('s'), None);
            }
        }
    }

    #[test]
    fn labels_preserve_neighboring_link_spaces() {
        let mut links = links(3);
        links[0].column = 0;
        links[1].id = links[0].id;
        links[1].target = links[0].target.clone();
        links[1].column = 0;
        links[1].width = 6;
        links[2].line = 1;
        links[2].column = 6;
        links[2].width = 3;
        let mut jump = Jump::new(&links, 0..2, 9).unwrap();
        let buffer = draw(&mut jump, Rect::new(5, 2, 9, 2), 0, "link\nabc   def");
        assert_eq!(buffer[(10, 3)].symbol(), " ");
        assert_eq!(buffer[(12, 3)].symbol(), "s");
        assert_eq!(jump.key('s'), Some(links[2].target.clone()));
    }

    #[test]
    fn adjacent_labels_remain_selectable() {
        let mut links = links(12);
        for (index, link) in links.iter_mut().enumerate() {
            link.line = 0;
            link.column = index * 4;
            link.width = 4;
        }
        let text = "xx界".repeat(links.len());
        let mut jump = Jump::new(&links, 0..1, 48).unwrap();
        let buffer = draw(&mut jump, Rect::new(5, 2, 48, 1), 0, &text);
        for (index, link) in links.iter().enumerate() {
            let column = 6 + index as u16 * 4;
            assert_eq!(
                buffer[(column, 2)].symbol(),
                char::from(ALPHABET[index / ALPHABET.len()]).to_string()
            );
            assert_eq!(
                buffer[(column + 1, 2)].symbol(),
                char::from(ALPHABET[index % ALPHABET.len()]).to_string()
            );
            assert_eq!(buffer[(column + 2, 2)].symbol(), " ");
            let mut chosen = Jump::new(&links, 0..1, 48).unwrap();
            draw(&mut chosen, Rect::new(5, 2, 48, 1), 0, &text);
            assert_eq!(
                chosen.key(char::from(ALPHABET[index / ALPHABET.len()])),
                None
            );
            assert_eq!(
                chosen.key(char::from(ALPHABET[index % ALPHABET.len()])),
                Some(link.target.clone())
            );
        }
    }
}
