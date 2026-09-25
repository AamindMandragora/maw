use super::app::{App, Hits, Popup, TABS, Tab};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use crate::style::{self, Tone};

// a tone from maw's palette as a terminal color
fn color(tone: Tone) -> Color {
    let (r, g, b) = style::rgb(tone);
    Color::Rgb(r, g, b)
}

fn toned(tone: Option<Tone>) -> Style {
    tone.map_or(Style::new(), |tone| Style::new().fg(color(tone)))
}

// accent text that names something: titles and key letters
fn accent() -> Style {
    Style::new().fg(color(Tone::Accent)).add_modifier(Modifier::BOLD)
}

// below this width previews go under the list instead of beside it
const WIDE: u16 = 90;

// the current tab and the selected row: void's green behind white
fn selected() -> Style {
    let (r, g, b) = style::GREEN;
    Style::new().bg(Color::Rgb(r, g, b)).fg(Color::White).add_modifier(Modifier::BOLD)
}

// a panel: dim border, accent title
fn block(title: String) -> Block<'static> {
    Block::new().borders(Borders::ALL).border_style(toned(Some(Tone::Dim))).title(Span::styled(title, accent()))
}

// the whole screen: tab bar, the tab, output if shown, footer, and any popup on top; the bar and footer take as many rows as they wrap to
pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let tab_rows = wrap(tab_labels(app, area.width), area.width);
    let footer_rows = footer_lines(app, area.width);
    let output = if app.show_output { (area.height / 3).clamp(4, 12) } else { 0 };
    let heights = [Constraint::Length(tab_rows.len() as u16), Constraint::Min(3), Constraint::Length(output), Constraint::Length(footer_rows.len() as u16)];
    let [bar, body, pane, footer] = Layout::vertical(heights).areas(area);

    draw_bar(frame, app, bar, tab_rows);
    draw_tab(frame, app, body);
    if app.show_output {
        draw_output(frame, app, pane);
    }
    frame.render_widget(Paragraph::new(footer_rows), footer);
    if let Some(popup) = &app.popup {
        draw_popup(frame, popup, area);
    }
}

// short names when the full ones don't fit
fn short(tab: Tab) -> &'static str {
    match tab {
        Tab::Packages => "pkgs",
        Tab::Modules => "mods",
        Tab::Services => "svc",
        Tab::Status => "stat",
        Tab::Diff => "diff",
        Tab::History => "hist",
        Tab::Git => "git",
    }
}

// " 1 packages ", or short names when the full ones don't fit on one row
fn tab_labels(app: &App, width: u16) -> Vec<Line<'static>> {
    let label = |index: usize, name: &str| format!(" {} {name} ", index + 1);
    let full: usize = TABS.iter().enumerate().map(|(index, tab)| label(index, tab.title()).chars().count()).sum();
    let name = |tab: Tab| if full <= width as usize { tab.title() } else { short(tab) };
    TABS.iter()
        .enumerate()
        .map(|(index, tab)| {
            let text = label(index, name(*tab));
            Line::from(if index == app.tab { Span::styled(text, selected()) } else { Span::raw(text) })
        })
        .collect()
}

// pieces (a tab, a key hint) laid out left to right, starting a new row whenever the next wouldn't fit; a piece is never split
fn wrap(pieces: Vec<Line<'static>>, width: u16) -> Vec<Vec<Line<'static>>> {
    pieces.into_iter().fold(vec![Vec::new()], |mut rows, piece| {
        let used: usize = rows.last().unwrap().iter().map(Line::width).sum();
        if used > 0 && used + piece.width() > width as usize {
            rows.push(Vec::new());
        }
        rows.last_mut().unwrap().push(piece);
        rows
    })
}

// a row of pieces as one line
fn joined(row: Vec<Line<'static>>) -> Line<'static> {
    Line::from(row.into_iter().flat_map(|piece| piece.spans).collect::<Vec<_>>())
}

// the tab rows, recording where each tab landed for clicks
fn draw_bar(frame: &mut Frame, app: &App, area: Rect, rows: Vec<Vec<Line<'static>>>) {
    let mut hits = app.hits.borrow_mut();
    hits.tabs.clear();
    rows.iter().enumerate().for_each(|(row, pieces)| {
        pieces.iter().fold(area.x, |x, piece| {
            hits.tabs.push((area.y + row as u16, x, x + piece.width() as u16));
            x + piece.width() as u16
        });
    });
    frame.render_widget(Paragraph::new(rows.into_iter().map(joined).collect::<Vec<_>>()), area);
}

// a list, with a preview beside or below it on tabs that have one; diff is scrollable text
fn draw_tab(frame: &mut Frame, app: &App, area: Rect) {
    let tab = app.current_tab();
    let title = match (&app.finding, tab) {
        (Some(term), Tab::Packages) => format!(" find: {term} "),
        _ => format!(" {} ", tab.title()),
    };

    if tab == Tab::Diff {
        let lines: Vec<Line> = app.state().rows.iter().map(|row| diff_line(&row.cells[0])).collect();
        let scroll = app.state().selected as u16;
        frame.render_widget(Paragraph::new(lines).block(block(title)).scroll((scroll, 0)), area);
        return;
    }

    // side by side when wide, stacked when narrow, list alone when there's no room
    let has_preview = matches!(tab, Tab::Modules | Tab::Services) && area.height >= 12;
    let (list_area, preview_area) = match (has_preview, area.width >= WIDE) {
        (false, _) => (area, None),
        (true, true) => {
            let [list, preview] = Layout::horizontal([Constraint::Percentage(40), Constraint::Fill(1)]).areas(area);
            (list, Some(preview))
        }
        (true, false) => {
            let [list, preview] = Layout::vertical([Constraint::Percentage(50), Constraint::Fill(1)]).areas(area);
            (list, Some(preview))
        }
    };
    draw_list(frame, app, list_area, block(title));
    if let Some(preview_area) = preview_area {
        let title = if tab == Tab::Services { " log " } else { " preview " };
        frame.render_widget(Paragraph::new(app.preview.join("\n")).block(block(title.into())), preview_area);
    }
}

// the tab's rows as aligned columns, the selection green; records the list's place and scroll for clicks
fn draw_list(frame: &mut Frame, app: &App, area: Rect, block: Block) {
    let state = app.state();
    let rows = state.visible();
    if !state.loaded {
        frame.render_widget(Paragraph::new("loading").block(block), area);
        return;
    }

    // each column as wide as its widest cell, the last one free
    let columns = rows.iter().map(|row| row.cells.len()).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns).map(|column| rows.iter().filter_map(|row| row.cells.get(column)).map(|cell| cell.chars().count()).max().unwrap_or(0)).collect();
    let items: Vec<ListItem> = rows
        .iter()
        .map(|row| {
            let cells = row.cells.iter().enumerate().map(|(column, cell)| if column + 1 < row.cells.len() { format!("{cell:<width$}  ", width = widths[column]) } else { cell.clone() });
            ListItem::new(cells.collect::<String>()).style(toned(row.tone))
        })
        .collect();

    let list = List::new(items).block(block).highlight_style(selected());
    let mut list_state = ListState::default().with_selected((!rows.is_empty()).then_some(state.selected));
    frame.render_stateful_widget(list, area, &mut list_state);
    let mut hits = app.hits.borrow_mut();
    *hits = Hits { list: area, offset: list_state.offset(), ..hits.clone() };
}

// a diff line in the same tones `maw diff` uses on a terminal
fn diff_line(line: &str) -> Line<'_> {
    Line::styled(line, toned(style::diff_tone(line)))
}

// the last lines the running or last command printed
fn draw_output(frame: &mut Frame, app: &App, area: Rect) {
    let title = match &app.busy {
        Some(label) => format!(" {label} "),
        None => " output ".into(),
    };
    let visible = area.height.saturating_sub(2) as usize;
    let lines: Vec<Line> = app.output[app.output.len().saturating_sub(visible)..].iter().map(|line| Line::styled(line.as_str(), toned(style::output_tone(line)))).collect();
    frame.render_widget(Paragraph::new(lines).block(block(title)), area);
}

// "i install  r remove" as one line per hint, each key letter green
fn hints(keys: &str) -> Vec<Line<'static>> {
    keys.split("  ")
        .map(|hint| {
            let (key, action) = hint.split_once(' ').unwrap_or((hint, ""));
            Line::from(vec![Span::styled(key.to_string(), accent()), Span::raw(format!(" {action}  "))])
        })
        .collect()
}

// the filter being typed, else what's running and the tab's keys, wrapped between hints to the width
fn footer_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    if app.filtering {
        let text: Vec<char> = format!("/{}", app.state().filter).chars().collect();
        return text.chunks(width.max(1) as usize).map(|chunk| Line::from(chunk.iter().collect::<String>())).collect();
    }
    let busy = app.busy.as_ref().map(|label| Line::from(Span::styled(format!("{label}  "), toned(Some(Tone::Accent)))));
    let pieces: Vec<Line> = busy.into_iter().chain(hints(app.current_tab().keys())).chain(hints("/ filter  ? keys  q quit")).collect();
    wrap(pieces, width).into_iter().map(joined).collect()
}

// a box in the middle of the screen
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect { x: area.x + (area.width - width) / 2, y: area.y + (area.height - height) / 2, width, height }
}

fn draw_popup(frame: &mut Frame, popup: &Popup, area: Rect) {
    let (title, body) = match popup {
        Popup::Input { title, input, .. } => (title.clone(), format!("{input}_")),
        Popup::Confirm { question, .. } => (String::new(), format!("{question} [y/n]")),
        Popup::Help => (String::from("keys"), HELP.into()),
    };
    let height = body.lines().count() as u16 + 2;
    let width = body.lines().chain([title.as_str()]).map(|line| line.chars().count() as u16 + 4).max().unwrap_or(20).max(40);
    let rect = centered(area, width, height);
    let block = Block::new().borders(Borders::ALL).border_style(toned(Some(Tone::Accent))).title(Span::styled(format!(" {title} "), accent()));
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }).block(block), rect);
}

const HELP: &str = "j k ↑ ↓      move          g G      top, bottom
h l ← → 1-7  switch tabs   /        filter
click        select        wheel    scroll
o            output        R        reload
q            quit

packages   i install  r remove  f find  s sync  A adopt
modules    e edit  n new  a add file
services   e enable  d disable  r restart  s status
status     a activate  F activate --force
diff       a activate
history    r roll back
git        c commit  p push  P pull";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::Row;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    // what the screen reads as, row by row
    fn screen(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height).map(|y| (0..buffer.area.width).map(|x| buffer[(x, y)].symbol().to_string()).collect::<String>() + "\n").collect()
    }

    #[test]
    fn tabs_rows_and_keys_are_drawn() {
        let mut app = App::new();
        app.set_rows(Tab::Packages, vec![Row::new(vec!["foot".into(), "1.28.0_1".into(), "xbps".into()], "foot")]);
        let text = screen(&app, 100, 20);
        assert!(text.contains("1 packages") && text.contains("7 git"));
        assert!(text.contains("foot  1.28.0_1  xbps"));
        assert!(text.contains("i install"));
    }

    #[test]
    fn narrow_screens_shorten_tabs_and_stack_previews() {
        let mut app = App::new();
        app.tab = 1;
        app.set_rows(Tab::Modules, vec![Row::new(vec!["niri".into()], "niri")]);
        app.preview = vec!["binds {".into()];
        let text = screen(&app, 60, 24);
        assert!(text.contains("1 pkgs") && text.contains("2 mods"));
        let lines: Vec<&str> = text.lines().collect();
        let list_row = lines.iter().position(|line| line.contains("niri")).unwrap();
        let preview_row = lines.iter().position(|line| line.contains("binds {")).unwrap();
        assert!(preview_row > list_row + 2);
    }

    #[test]
    fn tab_clicks_are_recorded_where_drawn() {
        let mut app = App::new();
        app.set_rows(Tab::Packages, Vec::new());
        screen(&app, 100, 20);
        let hits = app.hits.borrow();
        assert_eq!(hits.tabs.len(), 7);
        assert_eq!(hits.tabs[0], (0, 0, " 1 packages ".len() as u16));
    }

    #[test]
    fn tight_windows_wrap_tabs_and_keys_instead_of_cutting_them() {
        let mut app = App::new();
        app.set_rows(Tab::Packages, Vec::new());
        let text = screen(&app, 26, 20);
        (1..=7).for_each(|number| assert!(text.contains(&format!(" {number} ")), "tab {number}"));
        assert!(text.contains("A adopt") && text.contains("q quit"));

        // tabs on the second row are still clickable there
        let hits = app.hits.borrow();
        assert!(hits.tabs.iter().any(|(row, _, _)| *row == 1));
    }

    #[test]
    fn popups_and_output_show() {
        let mut app = App::new();
        app.set_rows(Tab::Packages, Vec::new());
        app.show_output = true;
        app.output = vec!["install foot".into()];
        app.busy = Some("install foot".into());
        app.ask("commit message [generation 3: 1 link]: ");
        let text = screen(&app, 100, 20);
        assert!(text.contains("install foot") && text.contains("commit message [generation 3: 1 link]:"));
    }

    #[test]
    fn rows_take_their_tone_and_the_selection_its_green() {
        let mut app = App::new();
        app.tab = 3;
        let lines = ["new        ~/a", "edited     ~/b"];
        app.set_rows(Tab::Status, lines.iter().map(|line| Row::text(*line).toned(style::status_tone(line))).collect());
        let mut terminal = Terminal::new(TestBackend::new(60, 12)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer().clone();

        // row one is selected, row two keeps its warn tone
        let (row_one, row_two) = (2, 3);
        let (r, g, b) = style::GREEN;
        assert_eq!(buffer[(1, row_one)].bg, Color::Rgb(r, g, b));
        assert_eq!(buffer[(1, row_two)].fg, color(Tone::Warn));
    }

    #[test]
    fn unloaded_tabs_say_so() {
        let app = App::new();
        assert!(screen(&app, 100, 20).contains("loading"));
    }
}
