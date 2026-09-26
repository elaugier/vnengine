//! Rich text parser for inline formatting tags.
//!
//! Supports Ren'Py-like text tags:
//! - `{b}...{/b}` — bold
//! - `{i}...{/i}` — italic
//! - `{u}...{/u}` — underline
//! - `{s}...{/s}` — strikethrough
//! - `{color=#ff0000}...{/color}` — text color (hex or named)
//! - `{size=20}...{/size}` — font size in pixels
//! - `{{` → literal `{`
//! - `}}` → literal `}`

/// A segment of text with formatting attributes.
#[derive(Debug, Clone, PartialEq)]
pub struct RichSegment {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    /// Optional color as [r, g, b, a] in 0..1.
    pub color: Option<[f32; 4]>,
    /// Optional font size override in pixels.
    pub size: Option<f32>,
}

/// Active formatting state during parsing.
#[derive(Debug, Clone, Default)]
struct FormatState {
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
    color: Option<[f32; 4]>,
    size: Option<f32>,
}

impl FormatState {
    fn make_segment(&self, text: String) -> RichSegment {
        RichSegment {
            text,
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            strikethrough: self.strikethrough,
            color: self.color,
            size: self.size,
        }
    }
}

/// Parse a text string with formatting tags into a list of segments.
pub fn parse_rich_text(text: &str) -> Vec<RichSegment> {
    let chars: Vec<char> = text.chars().collect();
    let mut segments = Vec::new();
    let mut current_text = String::new();
    let mut state = FormatState::default();
    let mut i = 0;

    while i < chars.len() {
        // Escaped braces: {{ → {, }} → }
        if chars[i] == '{' && i + 1 < chars.len() && chars[i + 1] == '{' {
            current_text.push('{');
            i += 2;
            continue;
        }
        if chars[i] == '}' && i + 1 < chars.len() && chars[i + 1] == '}' {
            current_text.push('}');
            i += 2;
            continue;
        }

        // Tag start
        if chars[i] == '{' {
            // Find closing }
            if let Some(close) = chars[i + 1..].iter().position(|&c| c == '}') {
                let tag: String = chars[i + 1..i + 1 + close].iter().collect();
                // Flush current text as a segment.
                if !current_text.is_empty() {
                    segments.push(state.make_segment(std::mem::take(&mut current_text)));
                }
                apply_tag(&mut state, &tag);
                i += 1 + close + 1;
                continue;
            } else {
                // No closing brace: literal
                current_text.push('{');
                i += 1;
                continue;
            }
        }

        current_text.push(chars[i]);
        i += 1;
    }

    // Flush remaining text.
    if !current_text.is_empty() {
        segments.push(state.make_segment(current_text));
    }

    // Merge adjacent segments with identical formatting.
    merge_adjacent(segments)
}

/// Apply a tag to the formatting state.
fn apply_tag(state: &mut FormatState, tag: &str) {
    let tag = tag.trim();
    if tag == "b" {
        state.bold = true;
    } else if tag == "/b" {
        state.bold = false;
    } else if tag == "i" {
        state.italic = true;
    } else if tag == "/i" {
        state.italic = false;
    } else if tag == "u" {
        state.underline = true;
    } else if tag == "/u" {
        state.underline = false;
    } else if tag == "s" {
        state.strikethrough = true;
    } else if tag == "/s" {
        state.strikethrough = false;
    } else if tag == "/color" {
        state.color = None;
    } else if tag == "/size" {
        state.size = None;
    } else if let Some(rest) = tag.strip_prefix("color=") {
        state.color = parse_color(rest);
    } else if let Some(rest) = tag.strip_prefix("size=") {
        if let Ok(size) = rest.trim().parse::<f32>() {
            state.size = Some(size);
        }
    }
    // Unknown tags are silently ignored.
}

/// Parse a color string: `#rrggbb`, `#rrggbbaa`, or named colors.
fn parse_color(s: &str) -> Option<[f32; 4]> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        match hex.len() {
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0])
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
                Some([
                    r as f32 / 255.0,
                    g as f32 / 255.0,
                    b as f32 / 255.0,
                    a as f32 / 255.0,
                ])
            }
            _ => None,
        }
    } else {
        // Named colors
        match s.to_ascii_lowercase().as_str() {
            "red" => Some([1.0, 0.0, 0.0, 1.0]),
            "green" => Some([0.0, 1.0, 0.0, 1.0]),
            "blue" => Some([0.0, 0.0, 1.0, 1.0]),
            "white" => Some([1.0, 1.0, 1.0, 1.0]),
            "black" => Some([0.0, 0.0, 0.0, 1.0]),
            "yellow" => Some([1.0, 1.0, 0.0, 1.0]),
            "cyan" => Some([0.0, 1.0, 1.0, 1.0]),
            "magenta" => Some([1.0, 0.0, 1.0, 1.0]),
            "gray" | "grey" => Some([0.5, 0.5, 0.5, 1.0]),
            "orange" => Some([1.0, 0.5, 0.0, 1.0]),
            "purple" => Some([0.5, 0.0, 0.5, 1.0]),
            _ => None,
        }
    }
}

/// Merge adjacent segments with identical formatting into one.
fn merge_adjacent(segments: Vec<RichSegment>) -> Vec<RichSegment> {
    let mut merged: Vec<RichSegment> = Vec::new();
    for seg in segments {
        if let Some(last) = merged.last_mut() {
            if last.bold == seg.bold
                && last.italic == seg.italic
                && last.underline == seg.underline
                && last.strikethrough == seg.strikethrough
                && last.color == seg.color
                && last.size == seg.size
            {
                last.text.push_str(&seg.text);
                continue;
            }
        }
        merged.push(seg);
    }
    merged
}

/// Strip all formatting tags from a text string, returning plain text.
/// This is used for the typewriter effect: we need to know the visible
/// character count without counting tag characters.
pub fn strip_tags(text: &str) -> String {
    let segments = parse_rich_text(text);
    segments.into_iter().map(|s| s.text).collect()
}

/// Count the number of visible characters (excluding tag markup).
pub fn visible_char_count(text: &str) -> usize {
    strip_tags(text).chars().count()
}

/// Extract the first `n` visible characters as rich text segments.
/// Returns segments where each segment's text is truncated to contribute
/// at most the remaining visible characters.
pub fn take_visible_chars(text: &str, n: usize) -> Vec<RichSegment> {
    let segments = parse_rich_text(text);
    let mut result = Vec::new();
    let mut remaining = n;

    for seg in segments {
        if remaining == 0 {
            break;
        }
        let char_count = seg.text.chars().count();
        if char_count <= remaining {
            result.push(seg);
            remaining -= char_count;
        } else {
            let truncated: String = seg.text.chars().take(remaining).collect();
            let mut partial = seg;
            partial.text = truncated;
            result.push(partial);
            remaining = 0;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plain_text() {
        let segs = parse_rich_text("Hello world");
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "Hello world");
        assert!(!segs[0].bold);
    }

    #[test]
    fn parse_bold() {
        let segs = parse_rich_text("Hello {b}world{/b}!");
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].text, "Hello ");
        assert!(!segs[0].bold);
        assert_eq!(segs[1].text, "world");
        assert!(segs[1].bold);
        assert_eq!(segs[2].text, "!");
        assert!(!segs[2].bold);
    }

    #[test]
    fn parse_italic() {
        let segs = parse_rich_text("{i}italic{/i} text");
        assert_eq!(segs.len(), 2);
        assert!(segs[0].italic);
        assert!(!segs[1].italic);
    }

    #[test]
    fn parse_color_hex() {
        let segs = parse_rich_text("{color=#ff0000}red{/color} normal");
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].color, Some([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(segs[1].color, None);
    }

    #[test]
    fn parse_color_named() {
        let segs = parse_rich_text("{color=blue}blue{/color}");
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].color, Some([0.0, 0.0, 1.0, 1.0]));
    }

    #[test]
    fn parse_size() {
        let segs = parse_rich_text("{size=20}big{/size} normal");
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].size, Some(20.0));
        assert_eq!(segs[1].size, None);
    }

    #[test]
    fn parse_nested_bold_italic() {
        let segs = parse_rich_text("{b}bold {i}both{/i}{/b}");
        assert_eq!(segs.len(), 2);
        assert!(segs[0].bold && !segs[0].italic);
        assert!(segs[1].bold && segs[1].italic);
    }

    #[test]
    fn parse_escaped_braces() {
        let segs = parse_rich_text("Literal {{braces}} here");
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "Literal {braces} here");
    }

    #[test]
    fn parse_unknown_tag_ignored() {
        let segs = parse_rich_text("{unknown}text{/unknown}");
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "text");
    }

    #[test]
    fn strip_tags_removes_markup() {
        assert_eq!(strip_tags("Hello {b}world{/b}!"), "Hello world!");
        assert_eq!(strip_tags("{color=red}red{/color}"), "red");
    }

    #[test]
    fn visible_char_count_excludes_tags() {
        assert_eq!(visible_char_count("Hello {b}world{/b}!"), 12);
        assert_eq!(visible_char_count("{size=20}ab{/size}"), 2);
    }

    #[test]
    fn take_visible_chars_partial() {
        let segs = take_visible_chars("Hello {b}world{/b}!", 7);
        let text: String = segs.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "Hello w");
        assert!(segs[1].bold); // "w" is inside the bold tag
    }

    #[test]
    fn take_visible_chars_full() {
        let segs = take_visible_chars("Hello {b}world{/b}!", 12);
        let text: String = segs.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "Hello world!");
    }

    #[test]
    fn take_visible_chars_zero() {
        let segs = take_visible_chars("Hello {b}world{/b}!", 0);
        assert!(segs.is_empty());
    }

    #[test]
    fn parse_underline_strikethrough() {
        let segs = parse_rich_text("{u}under{/u} {s}strike{/s}");
        assert_eq!(segs.len(), 3);
        assert!(segs[0].underline);
        assert!(segs[2].strikethrough);
    }

    #[test]
    fn parse_color_with_alpha() {
        let segs = parse_rich_text("{color=#ff000080}semi{/color}");
        let color = segs[0].color.unwrap();
        assert!((color[0] - 1.0).abs() < 0.01);
        assert!((color[1] - 0.0).abs() < 0.01);
        assert!((color[2] - 0.0).abs() < 0.01);
        assert!((color[3] - 128.0 / 255.0).abs() < 0.01);
    }

    #[test]
    fn merge_adjacent_same_format() {
        let segs = parse_rich_text("{b}a{/b}{b}b{/b}");
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "ab");
        assert!(segs[0].bold);
    }
}
