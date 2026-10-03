use crate::style::{Tone, paint};

// one chapter of the manual, compiled in so help always matches the installed maw; its number is its place in CHAPTERS
pub struct Chapter {
    pub name: &'static str,
    pub summary: &'static str,
    pub text: &'static str,
}

pub const CHAPTERS: [Chapter; 15] = [
    Chapter { name: "introduction", summary: "what maw is, getting help, the tui", text: include_str!("../docs/manual/01-introduction.md") },
    Chapter { name: "getting-started", summary: "making a repo, building, activating", text: include_str!("../docs/manual/02-getting-started.md") },
    Chapter { name: "day-to-day", summary: "writing, editing, adding, and checking config", text: include_str!("../docs/manual/03-day-to-day.md") },
    Chapter { name: "modules", summary: "writing modules: lib.program, lib.service, config.nix", text: include_str!("../docs/manual/04-modules.md") },
    Chapter { name: "formats", summary: "every output format and how nix values map to it", text: include_str!("../docs/manual/05-formats.md") },
    Chapter { name: "packages", summary: "installing, removing, finding, updating, adopting", text: include_str!("../docs/manual/06-packages.md") },
    Chapter { name: "source-packages", summary: "templates of your own, drafted from nixpkgs or the aur", text: include_str!("../docs/manual/07-source-packages.md") },
    Chapter { name: "services", summary: "runit services for the system and your session", text: include_str!("../docs/manual/08-services.md") },
    Chapter { name: "secrets", summary: "encrypted files, so the repo can be public", text: include_str!("../docs/manual/09-secrets.md") },
    Chapter { name: "themes", summary: "colors from your wallpaper", text: include_str!("../docs/manual/10-themes.md") },
    Chapter { name: "machines", summary: "one repo for several machines", text: include_str!("../docs/manual/11-machines.md") },
    Chapter { name: "history", summary: "generations, rolling back, sharing", text: include_str!("../docs/manual/12-history.md") },
    Chapter { name: "migrating", summary: "moving an existing setup into maw, step by step", text: include_str!("../docs/manual/13-migrating.md") },
    Chapter { name: "troubleshooting", summary: "doctor, logs, and where to look", text: include_str!("../docs/manual/14-troubleshooting.md") },
    Chapter { name: "reference", summary: "where files go, and what maw keeps", text: include_str!("../docs/manual/15-reference.md") },
];

// a chapter by number (`4`) or name (`source packages` or `source-packages`)
pub fn chapter(query: &str) -> Option<&'static Chapter> {
    let query = query.trim().to_lowercase().replace(' ', "-");
    match query.parse::<usize>() {
        Ok(number) => CHAPTERS.get(number.checked_sub(1)?),
        Err(_) => CHAPTERS.iter().find(|chapter| chapter.name == query),
    }
}

// a whole chapter, else the section whose heading best matches: exact, then prefix, then substring
pub fn find(query: &str) -> Option<String> {
    if let Some(chapter) = chapter(query) {
        return Some(chapter.text.to_string());
    }
    let query = query.trim().to_lowercase();

    // every heading in every chapter, as (normalized title, chapter text, line index)
    let headings: Vec<(String, &str, usize)> = CHAPTERS
        .iter()
        .flat_map(|chapter| headings(chapter.text).into_iter().map(|(index, _, title)| (title, chapter.text, index)))
        .collect();

    let matchers: [&dyn Fn(&str) -> bool; 3] = [&|title| title == query, &|title| title.starts_with(&query), &|title| title.contains(&query)];
    matchers
        .iter()
        .find_map(|matches| headings.iter().find(|(title, _, _)| matches(title)))
        .map(|(_, text, index)| section(text, *index))
}

// (level, lowercase title without backticks) of a markdown heading line
fn heading(line: &str) -> Option<(usize, String)> {
    let level = line.chars().take_while(|char| *char == '#').count();
    let title = line.get(level..)?.strip_prefix(' ')?;
    (level > 0).then(|| (level, title.replace('`', "").to_lowercase()))
}

// (line index, level, title) of every heading outside ``` code blocks
fn headings(text: &str) -> Vec<(usize, usize, String)> {
    let mut in_code = false;
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            in_code ^= line.starts_with("```");
            let (level, title) = heading(line).filter(|_| !in_code)?;
            Some((index, level, title))
        })
        .collect()
}

// from the heading at index up to the next heading at the same or a higher level
fn section(text: &str, index: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let all = headings(text);
    let level = all.iter().find(|(at, _, _)| *at == index).map_or(1, |(_, level, _)| *level);
    let end = all.iter().find(|(at, other, _)| *at > index && *other <= level).map_or(lines.len(), |(at, _, _)| *at);
    lines[index..end].join("\n") + "\n"
}

// markdown for a terminal in maw's palette when color is on: accent headings and inline code, dim code blocks; links as plain text either way
pub fn render(markdown: &str, color: bool) -> String {
    let style = |tone: Tone, bold: bool, text: &str| if color { paint(tone, text, bold) } else { text.to_string() };
    let mut in_code = false;
    let mut just_closed = false;

    // each line styled by what kind it is; fences and table rules are dropped
    markdown
        .lines()
        .filter_map(|line| {
            // back-to-back code blocks, like an example and its output, get a blank line between them
            if line.starts_with("```") {
                in_code = !in_code;
                let separate = in_code && just_closed;
                just_closed = !in_code;
                return separate.then(|| "\n".to_string());
            }
            just_closed = false;
            if in_code {
                return Some(if line.is_empty() { "\n".into() } else { format!("    {}\n", style(Tone::Dim, false, line)) });
            }
            if line.starts_with("|-") || line.starts_with("|:") {
                return None;
            }
            Some(match heading(line) {
                Some(_) => format!("{}\n", style(Tone::Accent, true, &line.trim_start_matches('#').trim().replace('`', ""))),
                None => format!("{}\n", inline(line, color)),
            })
        })
        .collect()
}

// inline markup: `code` highlighted, **bold** styled, links reduced to their text
fn inline(line: &str, color: bool) -> String {
    let linked = links(line);
    let bolded = styled_spans(&linked, "**", color.then_some(&|part: &str| format!("\x1b[1m{part}\x1b[0m")));
    styled_spans(&bolded, "`", color.then_some(&|part: &str| paint(Tone::Accent, part, false)))
}

// styles text between pairs of marker, or without color leaves backticks and drops **
fn styled_spans(text: &str, marker: &str, styled: Option<&dyn Fn(&str) -> String>) -> String {
    let parts: Vec<&str> = text.split(marker).collect();
    if parts.len().is_multiple_of(2) {
        return text.to_string();
    }

    // odd parts sit between markers
    parts
        .iter()
        .enumerate()
        .map(|(index, part)| match (index % 2, styled) {
            (0, _) => part.to_string(),
            (_, Some(styled)) => styled(part),
            (_, None) if marker == "`" => format!("`{part}`"),
            (_, None) => part.to_string(),
        })
        .collect()
}

// [text](target): another doc becomes `maw help <name>`, a web link keeps its address, an anchor is just its text
pub(crate) fn links(line: &str) -> String {
    let Some(start) = line.find('[') else { return line.to_string() };
    let Some(middle) = line[start..].find("](").map(|offset| start + offset) else { return line.to_string() };
    let Some(end) = line[middle..].find(')').map(|offset| middle + offset) else { return line.to_string() };

    // a chapter (04-modules.md) by its name, a section in one (02-getting-started.md#drift) by its heading
    let text = &line[start + 1..middle];
    let target = &line[middle + 2..end];
    let page = target.split_once(".md").filter(|(page, _)| !page.contains('/') && !target.starts_with("http"));
    let replacement = match page {
        Some((page, "")) => format!("`maw help {}`", page.trim_start_matches(|char: char| char.is_ascii_digit() || char == '-').replace('-', " ")),
        Some((_, anchor)) => format!("{text} (`maw help {}`)", anchor.trim_start_matches('#').replace('-', " ")),
        None if target.starts_with("http") => format!("{text} ({target})"),
        None => text.to_string(),
    };
    format!("{}{replacement}{}", &line[..start], links(&line[end + 1..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chapters_are_found_by_number_or_name() {
        assert!(find("5").unwrap().starts_with("# 5. Formats"));
        assert!(find("source packages").unwrap().starts_with("# 7. Source packages"));
        assert!(find("source-packages").unwrap().starts_with("# 7. Source packages"));
        assert!(chapter("0").is_none() && chapter("16").is_none());
    }

    #[test]
    fn chapter_links_name_the_chapter_or_section() {
        assert_eq!(links("see [modules](04-modules.md) and [drift](02-getting-started.md#drift)"), "see `maw help modules` and drift (`maw help drift`)");
    }

    #[test]
    fn sections_stop_at_the_next_sibling_heading() {
        let drift = find("drift").unwrap();
        assert!(drift.starts_with("### Drift"));
        assert!(!drift.contains("## Where files go"));

        // a level-2 section keeps its level-3 children
        assert!(find("activating").unwrap().contains("### Loose static files"));
    }

    #[test]
    fn exact_headings_win_over_partial_ones() {
        assert!(find("ini").unwrap().starts_with("## ini"));
        assert!(find("loose").unwrap().starts_with("### Loose static files"));
        assert!(find("no such thing").is_none());
    }

    #[test]
    fn headings_inside_code_blocks_are_ignored() {
        let text = "## a\n```sh\n# not a heading\n```\nafter\n## b\n";
        assert_eq!(section(text, 0), "## a\n```sh\n# not a heading\n```\nafter\n");
    }

    #[test]
    fn plain_render_drops_markup_but_keeps_code() {
        let rendered = render("## Title\n\nuse **this** and `that`, see [formats.md](formats.md) or [drift](#drift)\n```nix\nx = 1;\n```\n|---|---|\n", false);
        assert_eq!(rendered, "Title\n\nuse this and `that`, see `maw help formats` or drift\n    x = 1;\n");
    }

    #[test]
    fn adjacent_code_blocks_stay_apart() {
        assert_eq!(render("```nix\na\n```\n```ini\nb\n```\n", false), "    a\n\n    b\n");
    }

    #[test]
    fn web_links_keep_their_address() {
        assert_eq!(links("see the [manual](https://example.org/Manual.md)."), "see the manual (https://example.org/Manual.md).");
    }

    #[test]
    fn color_render_styles_code_and_headings() {
        let rendered = render("## Title\n`x`\n", true);
        assert_eq!(rendered, format!("{}\n{}\n", paint(Tone::Accent, "Title", true), paint(Tone::Accent, "x", false)));
    }

    #[test]
    fn every_doc_link_points_at_a_chapter_or_section() {
        let is_doc_link = |line: &&str| line.contains(".md") && line.contains("](") && !line.contains("](http");
        CHAPTERS.iter().flat_map(|chapter| chapter.text.lines()).filter(is_doc_link).for_each(|line| {
            let rendered = links(line);
            rendered.split("maw help ").skip(1).for_each(|query| assert!(find(query.split('`').next().unwrap()).is_some(), "{line}"));
        });
    }

    #[test]
    fn every_chapter_is_numbered_in_order() {
        CHAPTERS.iter().enumerate().for_each(|(index, chapter)| assert!(chapter.text.starts_with(&format!("# {}. ", index + 1)), "{}", chapter.name));
    }
}
