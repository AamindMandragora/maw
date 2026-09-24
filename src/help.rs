// one user doc, compiled in so help always matches the installed maw
pub struct Topic {
    pub name: &'static str,
    pub summary: &'static str,
    pub text: &'static str,
}

pub const TOPICS: [Topic; 4] = [
    Topic { name: "usage", summary: "setting up, building, activating, day-to-day commands", text: include_str!("../docs/usage.md") },
    Topic { name: "migrating", summary: "moving an existing setup into maw, step by step", text: include_str!("../docs/migrating.md") },
    Topic { name: "modules", summary: "writing modules: lib.program, lib.raw, config.nix", text: include_str!("../docs/modules.md") },
    Topic { name: "formats", summary: "every output format and how nix values map to it", text: include_str!("../docs/formats.md") },
];

// a whole topic by name, else the section whose heading best matches: exact, then prefix, then substring
pub fn find(query: &str) -> Option<String> {
    let query = query.trim().to_lowercase();
    if let Some(topic) = TOPICS.iter().find(|topic| topic.name == query) {
        return Some(topic.text.to_string());
    }

    // every heading in every topic, as (normalized title, topic text, line index)
    let headings: Vec<(String, &str, usize)> = TOPICS
        .iter()
        .flat_map(|topic| headings(topic.text).into_iter().map(|(index, _, title)| (title, topic.text, index)))
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

// markdown for a terminal: styled headings, inline code, and code blocks when color is on; links as plain text either way
pub fn render(markdown: &str, color: bool) -> String {
    let style = |code: &str, text: &str| if color { format!("\x1b[{code}m{text}\x1b[0m") } else { text.to_string() };
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
                return Some(if line.is_empty() { "\n".into() } else { format!("    {}\n", style("2", line)) });
            }
            if line.starts_with("|-") || line.starts_with("|:") {
                return None;
            }
            // top-level headings are underlined as well as bold
            Some(match heading(line) {
                Some((level, _)) => format!("{}\n", style(if level == 1 { "1;4" } else { "1" }, &inline(line.trim_start_matches('#').trim(), color))),
                None => format!("{}\n", inline(line, color)),
            })
        })
        .collect()
}

// inline markup: `code` highlighted, **bold** styled, links reduced to their text
fn inline(line: &str, color: bool) -> String {
    let linked = links(line);
    let bolded = styled_spans(&linked, "**", if color { Some("1") } else { None });
    styled_spans(&bolded, "`", if color { Some("33") } else { None })
}

// wraps text between pairs of marker in an ansi style, or leaves backticks and drops ** without color
fn styled_spans(text: &str, marker: &str, code: Option<&str>) -> String {
    let parts: Vec<&str> = text.split(marker).collect();
    if parts.len().is_multiple_of(2) {
        return text.to_string();
    }

    // odd parts sit between markers
    parts
        .iter()
        .enumerate()
        .map(|(index, part)| match (index % 2, code) {
            (0, _) => part.to_string(),
            (_, Some(code)) => format!("\x1b[{code}m{part}\x1b[0m"),
            (_, None) if marker == "`" => format!("`{part}`"),
            (_, None) => part.to_string(),
        })
        .collect()
}

// [text](target): another doc becomes `maw help <name>`, a web link keeps its address, an anchor is just its text
fn links(line: &str) -> String {
    let Some(start) = line.find('[') else { return line.to_string() };
    let Some(middle) = line[start..].find("](").map(|offset| start + offset) else { return line.to_string() };
    let Some(end) = line[middle..].find(')').map(|offset| middle + offset) else { return line.to_string() };

    let text = &line[start + 1..middle];
    let target = &line[middle + 2..end];
    let replacement = match target.strip_suffix(".md").filter(|doc| !doc.contains('/')) {
        Some(doc) => format!("`maw help {doc}`"),
        None if target.starts_with("http") => format!("{text} ({target})"),
        None => text.to_string(),
    };
    format!("{}{replacement}{}", &line[..start], links(&line[end + 1..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topics_are_found_by_name() {
        assert!(find("formats").unwrap().starts_with("# Formats"));
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
        assert_eq!(rendered, "\x1b[1mTitle\x1b[0m\n\x1b[33mx\x1b[0m\n");
    }

    #[test]
    fn every_doc_link_points_at_a_topic() {
        let is_topic_link = |line: &&str| line.contains(".md)") && !line.contains("](http");
        TOPICS.iter().flat_map(|topic| topic.text.lines()).filter(is_topic_link).for_each(|line| {
            let rendered = links(line);
            let doc = rendered.split("maw help ").nth(1).unwrap().split('`').next().unwrap();
            assert!(find(doc).is_some(), "{line}");
        });
    }
}
