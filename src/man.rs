use crate::cli;
use crate::help::{TOPICS, links};

// man pages: maw(1) from the command tree, the user docs as maw-<topic>(5 or 7)

// which section each doc topic belongs in: file formats are 5, guides are 7
fn section(topic: &str) -> u8 {
    if matches!(topic, "modules" | "formats") { 5 } else { 7 }
}

// every page as (file name, roff)
pub fn pages() -> Vec<(String, String)> {
    let docs = TOPICS.iter().map(|topic| {
        let section = section(topic.name);
        let name = format!("maw-{}", topic.name);
        (format!("{name}.{section}"), roff(&name, section, topic.summary, topic.text))
    });
    let mut root = cli::command();
    root.build();
    let commands = subcommand_pages(&root, "maw");
    std::iter::once(("maw.1".to_string(), command_page())).chain(commands).chain(docs).collect()
}

// maw-<command>(1) for every subcommand, and maw-<command>-<sub>(1) below it, the way maw(1) refers to them
fn subcommand_pages(command: &clap::Command, prefix: &str) -> Vec<(String, String)> {
    command
        .get_subcommands()
        .flat_map(|sub| {
            let name = format!("{prefix}-{}", sub.get_name());
            let mut page = Vec::new();
            clap_mangen::Man::new(sub.clone().name(name.clone())).render(&mut page).unwrap();
            std::iter::once((format!("{name}.1"), String::from_utf8(page).unwrap())).chain(subcommand_pages(sub, &name)).collect::<Vec<_>>()
        })
        .collect()
}

// maw(1): every command and flag, then pointers to the docs pages
fn command_page() -> String {
    let mut page = Vec::new();
    clap_mangen::Man::new(cli::command()).render(&mut page).unwrap();
    let see_also: Vec<String> = TOPICS.iter().map(|topic| format!("\\fBmaw-{}\\fR({})", topic.name, section(topic.name))).collect();
    format!("{}.SH \"SEE ALSO\"\n{}\n", String::from_utf8(page).unwrap(), see_also.join(", "))
}

// a line as roff text: backslashes escaped, and no leading dot or quote that roff would read as a request
fn escape(text: &str) -> String {
    let text = text.replace('\\', "\\e");
    if text.starts_with('.') || text.starts_with('\'') { format!("\\&{text}") } else { text }
}

// inline markdown: links as `maw help` shows them, `code` and **bold** in bold
fn inline(text: &str) -> String {
    let linked = escape(&links(text));
    ["**", "`"].iter().fold(linked, |text, marker| {
        let parts: Vec<&str> = text.split(marker).collect();
        if parts.len().is_multiple_of(2) {
            return text;
        }
        parts.iter().enumerate().map(|(index, part)| if index % 2 == 1 { format!("\\fB{part}\\fR") } else { part.to_string() }).collect()
    })
}

// a markdown table as aligned columns in a literal block, separator rows dropped
fn table(rows: &[&str]) -> String {
    let cells: Vec<Vec<String>> = rows
        .iter()
        .filter(|row| !row.starts_with("|-") && !row.starts_with("|:"))
        .map(|row| row.trim().trim_matches('|').split('|').map(|cell| links(cell.trim()).replace('`', "")).collect())
        .collect();
    let columns = cells.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns).map(|column| cells.iter().filter_map(|row| row.get(column)).map(|cell| cell.chars().count()).max().unwrap_or(0)).collect();
    let lines: Vec<String> = cells
        .iter()
        .map(|row| row.iter().enumerate().map(|(column, cell)| format!("{cell:<width$}", width = widths[column] + 2)).collect::<String>().trim_end().to_string())
        .map(|line| escape(&line))
        .collect();
    format!(".PP\n.EX\n{}\n.EE\n", lines.join("\n"))
}

// one markdown doc as a man page; the doc's own title gives way to NAME
pub fn roff(name: &str, section: u8, summary: &str, markdown: &str) -> String {
    let header = format!(".TH \"{}\" \"{section}\" \"\" \"maw {}\" \"maw manual\"\n.SH NAME\n{name} \\- {summary}\n", name.to_uppercase(), env!("CARGO_PKG_VERSION"));
    let lines: Vec<&str> = markdown.lines().collect();
    let mut body = String::new();
    let mut index = 0;

    // blocks: fenced code, tables, headings, list items, and paragraphs of plain lines
    while index < lines.len() {
        let line = lines[index];
        if line.starts_with("```") {
            let end = lines[index + 1..].iter().position(|line| line.starts_with("```")).map_or(lines.len(), |offset| index + 1 + offset);
            let code: Vec<String> = lines[index + 1..end].iter().map(|line| escape(line)).collect();
            body += &format!(".PP\n.EX\n{}\n.EE\n", code.join("\n"));
            index = end + 1;
        } else if line.starts_with('|') {
            let end = lines[index..].iter().position(|line| !line.starts_with('|')).map_or(lines.len(), |offset| index + offset);
            body += &table(&lines[index..end]);
            index = end;
        } else if let Some(title) = line.strip_prefix("## ") {
            body += &format!(".SH \"{}\"\n", title.replace('`', "").to_uppercase());
            index += 1;
        } else if let Some(title) = line.strip_prefix("### ") {
            body += &format!(".SS \"{}\"\n", title.replace('`', ""));
            index += 1;
        } else if let Some(item) = line.strip_prefix("- ") {
            body += &format!(".IP \\(bu 2\n{}\n", inline(item));
            index += 1;
        } else if let Some((number, item)) = line.split_once(". ").filter(|(number, _)| !number.is_empty() && number.chars().all(|char| char.is_ascii_digit())) {
            body += &format!(".IP {number}. 4\n{}\n", inline(item));
            index += 1;
        } else if line.starts_with("# ") || line.trim().is_empty() {
            index += 1;
        } else {
            // a paragraph runs until a blank line or another kind of block
            let end = lines[index..]
                .iter()
                .position(|line| line.trim().is_empty() || line.starts_with(['#', '|', '-']) || line.starts_with("```"))
                .map_or(lines.len(), |offset| index + offset);
            let text: Vec<String> = lines[index..end].iter().map(|line| inline(line)).collect();
            body += &format!(".PP\n{}\n", text.join("\n"));
            index = end.max(index + 1);
        }
    }
    // text before the first heading is the description
    let description = if body.starts_with(".SH") { "" } else { ".SH DESCRIPTION\n" };
    format!("{header}{description}{body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_becomes_roff() {
        let page = roff("maw-x", 7, "a test", "# X\n\nSome `code` and **bold**.\n\n## Usage\n\n```sh\nmaw x\n.hidden\n```\n\n- one\n- two\n\n| key | meaning |\n|---|---|\n| `a` | first |\n");
        assert!(page.starts_with(".TH \"MAW-X\" \"7\""));
        assert!(page.contains(".SH NAME\nmaw-x \\- a test\n.SH DESCRIPTION\n.PP\nSome"));
        assert!(page.contains("Some \\fBcode\\fR and \\fBbold\\fR."));
        assert!(page.contains(".SH \"USAGE\"\n.PP\n.EX\nmaw x\n\\&.hidden\n.EE\n"));
        assert!(page.contains(".IP \\(bu 2\none\n.IP \\(bu 2\ntwo\n"));
        assert!(page.contains("key  meaning\na    first"));
    }

    #[test]
    fn every_page_is_listed() {
        let names: Vec<String> = pages().into_iter().map(|(name, _)| name).collect();
        assert_eq!(names[0], "maw.1");
        assert!(names.contains(&"maw-install.1".to_string()) && names.contains(&"maw-sv-enable.1".to_string()));
        assert!(names.ends_with(&["maw-usage.7".into(), "maw-migrating.7".into(), "maw-modules.5".into(), "maw-formats.5".into()]));
    }

    #[test]
    fn every_page_maw_1_names_exists() {
        let all = pages();
        let names: Vec<&String> = all.iter().map(|(name, _)| name).collect();
        let referenced = all[0].1.lines().filter_map(|line| line.strip_suffix("(1)")).map(|name| format!("{}.1", name.replace("\\-", "-")));
        referenced.for_each(|page| assert!(names.contains(&&page), "{page}"));
    }
}
