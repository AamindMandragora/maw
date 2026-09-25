// maw's one palette, for the cli on a terminal and for the tui: each tone means the same thing everywhere

// what a piece of text is, which decides its color
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    // titles, key letters, things maw is about to do, additions
    Accent,
    // errors and removals
    Bad,
    // warnings and drift: things that need a look
    Warn,
    // borders, notes, context
    Dim,
}

// void linux's logo green, behind the selection
pub const GREEN: (u8, u8, u8) = (0x47, 0x80, 0x61);

// the lighter green, for accent text
pub const LIGHT: (u8, u8, u8) = (0x62, 0xb0, 0x86);

const RED: (u8, u8, u8) = (0xd0, 0x62, 0x62);
const AMBER: (u8, u8, u8) = (0xd7, 0xa6, 0x5f);
const GRAY: (u8, u8, u8) = (0x80, 0x80, 0x80);

pub fn rgb(tone: Tone) -> (u8, u8, u8) {
    match tone {
        Tone::Accent => LIGHT,
        Tone::Bad => RED,
        Tone::Warn => AMBER,
        Tone::Dim => GRAY,
    }
}

// text in a tone for a terminal; bold for the error and warning prefixes
pub fn paint(tone: Tone, text: &str, bold: bool) -> String {
    let (r, g, b) = rgb(tone);
    let weight = if bold { "1;" } else { "" };
    format!("\x1b[{weight}38;2;{r};{g};{b}m{text}\x1b[0m")
}

// a status label's tone: pending work is accent, drift and problems warn, things outside maw's reach are dim
pub fn label_tone(label: &str) -> Option<Tone> {
    match label {
        "new" | "changed" | "moved" | "restart" | "outdated" | "clean" => Some(Tone::Accent),
        "blocked" | "replaced" | "edited" | "missing" | "stale" | "disabled" => Some(Tone::Warn),
        "undeclared" | "orphan" | "unplaced" => Some(Tone::Dim),
        _ => None,
    }
}

// a diff line's tone: additions accent, removals bad, hunk headers dim; file headers stay plain
pub fn diff_tone(line: &str) -> Option<Tone> {
    match line.chars().next() {
        Some('+') if !line.starts_with("+++") => Some(Tone::Accent),
        Some('-') if !line.starts_with("---") => Some(Tone::Bad),
        Some('@') => Some(Tone::Dim),
        _ => None,
    }
}

// a status line ("label      text") in its label's tone
pub fn status_tone(line: &str) -> Option<Tone> {
    label_tone(line.split_whitespace().next()?)
}

// a line of command output: errors bad, warnings warn
pub fn output_tone(line: &str) -> Option<Tone> {
    if line.starts_with("error:") {
        Some(Tone::Bad)
    } else if line.starts_with("warning:") {
        Some(Tone::Warn)
    } else {
        None
    }
}

// a note like (missing) or (undeclared) in query and sv list
pub fn note_tone(note: &str) -> Option<Tone> {
    label_tone(note.trim().trim_matches(|char| char == '(' || char == ')'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tones_follow_meaning() {
        assert_eq!(status_tone("new        ~/.config/foot/foot.ini"), Some(Tone::Accent));
        assert_eq!(status_tone("edited     ~/.config/waybar/style.css"), Some(Tone::Warn));
        assert_eq!(status_tone("undeclared firefox"), Some(Tone::Dim));
        assert_eq!(diff_tone("+font=x"), Some(Tone::Accent));
        assert_eq!(diff_tone("--- a (live)"), None);
        assert_eq!(output_tone("warning: ~/go/bin isn't on PATH"), Some(Tone::Warn));
        assert_eq!(note_tone("  (missing)"), Some(Tone::Warn));
    }

    #[test]
    fn paint_is_truecolor_and_resets() {
        assert_eq!(paint(Tone::Bad, "error:", true), "\x1b[1;38;2;208;98;98merror:\x1b[0m");
    }
}
