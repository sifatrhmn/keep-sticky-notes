//! Simple text formatting stored as readable markers inside the note's plain text.
//! Google Keep's `text` field is plain, so markers survive sync and stay legible elsewhere:
//! `**bold**`, `*italic*`, `__underline__`, `~~strikethrough~~`, `# Heading`, `• bullet`.
//! All positions are UTF-16 code units, with a line break counted as one unit.

pub const BOLD: u8 = 1;
pub const ITALIC: u8 = 2;
pub const UNDERLINE: u8 = 4;
pub const STRIKE: u8 = 8;
pub const HEADING: u8 = 16;
pub const MARKER: u8 = 32;

pub const BULLET_PREFIX: &str = "\u{2022} ";
pub const HEADING_PREFIX: &str = "# ";

const NL: u16 = b'\n' as u16;
const SPACE: u16 = b' ' as u16;
const TAB: u16 = b'\t' as u16;

#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub start: usize,
    pub end: usize,
    pub flags: u8,
}

/// A single replacement to perform in the editor, and the selection to leave afterwards.
#[derive(Debug, Clone, PartialEq)]
pub struct TextEdit {
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub sel_start: usize,
    pub sel_end: usize,
}

fn u16s(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn is_space(c: u16) -> bool {
    c == SPACE || c == TAB
}

fn is_word(c: u16) -> bool {
    char::from_u32(c as u32).map(|c| c.is_alphanumeric()).unwrap_or(false)
}

fn matches_at(text: &[u16], pos: usize, pat: &[u16]) -> bool {
    text.get(pos..pos + pat.len()).map(|s| s == pat).unwrap_or(false)
}

fn free(flags: &[u8], pos: usize, len: usize) -> bool {
    flags[pos..pos + len].iter().all(|f| f & MARKER == 0)
}

fn mark_pair(text: &[u16], flags: &mut [u8], line: (usize, usize), pat: &[u16], style: u8, word_edges: bool) {
    let (start, end) = line;
    let m = pat.len();
    let single = m == 1; // A lone `*` must never be half of a `**` pair.
    let mut i = start;
    while i + 2 * m < end {
        let open_ok = matches_at(text, i, pat)
            && free(flags, i, m)
            && !is_space(text[i + m])
            && !(word_edges && i > start && is_word(text[i - 1]))
            && !(single && (text[i + 1] == pat[0] || (i > start && text[i - 1] == pat[0])));
        if !open_ok {
            i += 1;
            continue;
        }
        let mut j = i + m + 1;
        let mut close = None;
        while j + m <= end {
            let edge_ok = !word_edges || j + m == end || !is_word(text[j + m]);
            let lone = !single || (text[j - 1] != pat[0] && (j + 1 == end || text[j + 1] != pat[0]));
            if matches_at(text, j, pat) && free(flags, j, m) && !is_space(text[j - 1]) && edge_ok && lone {
                close = Some(j);
                break;
            }
            j += 1;
        }
        let Some(j) = close else {
            i += 1;
            continue;
        };
        flags[i..i + m].iter_mut().for_each(|f| *f |= MARKER);
        flags[j..j + m].iter_mut().for_each(|f| *f |= MARKER);
        flags[i + m..j].iter_mut().for_each(|f| *f |= style);
        i = j + m;
    }
}

fn character_flags(text: &[u16]) -> Vec<u8> {
    let mut flags = vec![0u8; text.len()];
    let heading = u16s(HEADING_PREFIX);
    let (bold, strike, under, ital) = (u16s("**"), u16s("~~"), u16s("__"), u16s("*"));
    let mut start = 0;
    while start <= text.len() {
        let end = text[start..].iter().position(|c| *c == NL).map(|p| start + p).unwrap_or(text.len());
        if matches_at(text, start, &heading) && end > start + heading.len() {
            flags[start..end].iter_mut().for_each(|f| *f |= HEADING);
            flags[start..start + heading.len()].iter_mut().for_each(|f| *f |= MARKER);
        }
        mark_pair(text, &mut flags, (start, end), &bold, BOLD, false);
        mark_pair(text, &mut flags, (start, end), &strike, STRIKE, false);
        mark_pair(text, &mut flags, (start, end), &under, UNDERLINE, true);
        mark_pair(text, &mut flags, (start, end), &ital, ITALIC, true);
        start = end + 1;
    }
    flags
}

/// Styled ranges of the text. Unstyled text produces no runs.
pub fn style_runs(text: &[u16]) -> Vec<Run> {
    let flags = character_flags(text);
    let mut runs = Vec::new();
    let mut i = 0;
    while i < flags.len() {
        let f = flags[i];
        let mut j = i + 1;
        while j < flags.len() && flags[j] == f {
            j += 1;
        }
        if f != 0 {
            runs.push(Run { start: i, end: j, flags: f });
        }
        i = j;
    }
    runs
}

/// The text without formatting markers, for previews.
pub fn strip_markers(s: &str) -> String {
    let text = u16s(s);
    let flags = character_flags(&text);
    let kept: Vec<u16> = text.iter().zip(&flags).filter(|(_, f)| **f & MARKER == 0).map(|(c, _)| *c).collect();
    String::from_utf16_lossy(&kept)
}

/// Adds or removes an inline marker around the selection.
pub fn toggle_wrap(text: &[u16], sel_start: usize, sel_end: usize, marker: &str) -> TextEdit {
    let pat = u16s(marker);
    let m = pat.len();
    let (mut s, mut e) = (sel_start.min(text.len()), sel_end.min(text.len()));
    if s == e {
        return TextEdit { start: s, end: s, text: format!("{marker}{marker}"), sel_start: s + m, sel_end: s + m };
    }
    while s < e && (is_space(text[s]) || text[s] == NL) {
        s += 1;
    }
    while e > s && (is_space(text[e - 1]) || text[e - 1] == NL) {
        e -= 1;
    }
    if s == e {
        return TextEdit { start: s, end: s, text: String::new(), sel_start: s, sel_end: s };
    }
    let inner = |a: usize, b: usize| String::from_utf16_lossy(&text[a..b]);
    if s >= m && matches_at(text, s - m, &pat) && matches_at(text, e, &pat) {
        return TextEdit { start: s - m, end: e + m, text: inner(s, e), sel_start: s - m, sel_end: e - m };
    }
    if e - s >= 2 * m && matches_at(text, s, &pat) && matches_at(text, e - m, &pat) {
        return TextEdit { start: s, end: e, text: inner(s + m, e - m), sel_start: s, sel_end: e - 2 * m };
    }
    let wrapped: Vec<String> = inner(s, e)
        .split('\n')
        .map(|l| if l.trim().is_empty() { l.to_string() } else { format!("{marker}{l}{marker}") })
        .collect();
    let joined = wrapped.join("\n");
    let len = joined.encode_utf16().count();
    TextEdit { start: s, end: e, text: joined, sel_start: s, sel_end: s + len }
}

/// Adds or removes a line prefix on every line touched by the selection.
pub fn toggle_line_prefix(text: &[u16], sel_start: usize, sel_end: usize, prefix: &str) -> TextEdit {
    let s = sel_start.min(text.len());
    let mut e = sel_end.min(text.len());
    if e > s && text[e - 1] == NL {
        e -= 1;
    }
    let block_start = text[..s].iter().rposition(|c| *c == NL).map(|p| p + 1).unwrap_or(0);
    let block_end = text[e..].iter().position(|c| *c == NL).map(|p| e + p).unwrap_or(text.len());
    let block = String::from_utf16_lossy(&text[block_start..block_end]);
    let lines: Vec<&str> = block.split('\n').collect();
    let all_prefixed = lines.iter().filter(|l| !l.trim().is_empty()).all(|l| l.starts_with(prefix))
        && lines.iter().any(|l| !l.trim().is_empty());
    let changed: Vec<String> = lines
        .iter()
        .map(|l| {
            if all_prefixed {
                l.strip_prefix(prefix).unwrap_or(l).to_string()
            } else if l.trim().is_empty() && lines.len() > 1 || l.starts_with(prefix) {
                l.to_string()
            } else {
                format!("{prefix}{l}")
            }
        })
        .collect();
    let joined = changed.join("\n");
    let len = joined.encode_utf16().count();
    let caret_only = sel_start == sel_end && lines.len() == 1;
    let (a, b) = if caret_only { (block_start + len, block_start + len) } else { (block_start, block_start + len) };
    TextEdit { start: block_start, end: block_end, text: joined, sel_start: a, sel_end: b }
}

/// What the Enter key should do inside a bulleted line, if anything special.
pub fn bullet_enter(text: &[u16], caret: usize) -> Option<TextEdit> {
    let caret = caret.min(text.len());
    let prefix = u16s(BULLET_PREFIX);
    let start = text[..caret].iter().rposition(|c| *c == NL).map(|p| p + 1).unwrap_or(0);
    if !matches_at(text, start, &prefix) {
        return None;
    }
    let end = text[caret..].iter().position(|c| *c == NL).map(|p| caret + p).unwrap_or(text.len());
    if end == start + prefix.len() {
        return Some(TextEdit { start, end, text: String::new(), sel_start: start, sel_end: start });
    }
    let insert = format!("\n{BULLET_PREFIX}");
    let pos = caret + insert.encode_utf16().count();
    Some(TextEdit { start: caret, end: caret, text: insert, sel_start: pos, sel_end: pos })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags_of(s: &str) -> Vec<u8> {
        character_flags(&u16s(s))
    }

    fn apply(s: &str, e: &TextEdit) -> String {
        let t = u16s(s);
        let mut out = t[..e.start].to_vec();
        out.extend(u16s(&e.text));
        out.extend_from_slice(&t[e.end..]);
        String::from_utf16_lossy(&out)
    }

    #[test]
    fn styles_each_marker() {
        let f = flags_of("**b** *i* __u__ ~~s~~");
        assert_eq!(f[2], BOLD);
        assert_eq!(f[0], MARKER);
        assert_eq!(f[7], ITALIC);
        assert_eq!(f[12], UNDERLINE);
        assert_eq!(f[18], STRIKE);
    }

    #[test]
    fn ordinary_text_is_left_alone() {
        for s in ["2*3*4 = 24", "snake__case__name", "price * 2 and * 3", "**unclosed", "a ** b ** c", ""] {
            assert!(style_runs(&u16s(s)).is_empty(), "{s}");
        }
    }

    #[test]
    fn markers_do_not_cross_lines() {
        assert!(style_runs(&u16s("**one\ntwo**")).is_empty());
    }

    #[test]
    fn heading_and_preview() {
        let f = flags_of("# Title\nbody");
        assert_eq!(f[0] & MARKER, MARKER);
        assert_eq!(f[3], HEADING);
        assert_eq!(f[8], 0);
        assert_eq!(strip_markers("# Title **now**"), "Title now");
    }

    #[test]
    fn non_ascii_offsets_are_correct() {
        let s = "😀 **ব**";
        let f = flags_of(s);
        assert_eq!(f[3..5], [MARKER, MARKER]);
        assert_eq!(f[5], BOLD);
    }

    #[test]
    fn wrap_and_unwrap_round_trip() {
        let s = "say hello now";
        let e = toggle_wrap(&u16s(s), 4, 9, "**");
        let wrapped = apply(s, &e);
        assert_eq!(wrapped, "say **hello** now");
        let back = toggle_wrap(&u16s(&wrapped), e.sel_start, e.sel_end, "**");
        assert_eq!(apply(&wrapped, &back), s);
    }

    #[test]
    fn wrap_excludes_surrounding_spaces_and_handles_lines() {
        let s = "one two";
        assert_eq!(apply(s, &toggle_wrap(&u16s(s), 3, 7, "*")), "one *two*");
        let m = "a\nb";
        assert_eq!(apply(m, &toggle_wrap(&u16s(m), 0, 3, "**")), "**a**\n**b**");
    }

    #[test]
    fn empty_selection_places_caret_between_markers() {
        let e = toggle_wrap(&u16s("x"), 1, 1, "**");
        assert_eq!((e.text.as_str(), e.sel_start), ("****", 3));
    }

    #[test]
    fn bullets_toggle_on_selected_lines() {
        let s = "one\ntwo\nthree";
        let t = u16s(s);
        let on = apply(s, &toggle_line_prefix(&t, 1, 5, BULLET_PREFIX));
        assert_eq!(on, "\u{2022} one\n\u{2022} two\nthree");
        let t2 = u16s(&on);
        let off = apply(&on, &toggle_line_prefix(&t2, 2, 8, BULLET_PREFIX));
        assert_eq!(off, s);
    }

    #[test]
    fn enter_continues_then_ends_a_list() {
        let s = "\u{2022} item";
        let e = bullet_enter(&u16s(s), 6).unwrap();
        let next = apply(s, &e);
        assert_eq!(next, "\u{2022} item\n\u{2022} ");
        let end = bullet_enter(&u16s(&next), e.sel_start).unwrap();
        assert_eq!(apply(&next, &end), "\u{2022} item\n");
        assert!(bullet_enter(&u16s("plain"), 2).is_none());
    }
}
