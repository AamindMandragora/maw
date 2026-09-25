use super::app::{App, Hits, Popup, TABS, Tab};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

// void linux's greens: the logo's, and a lighter one for text
const GREEN: Color = Color::Rgb(0x47, 0x80, 0x61);
const LIGHT: Color = Color::Rgb(0x62, 0xb0, 0x86);

// below this width previews go under the list instead of beside it
const WIDE: u16 = 90;

fn selected() -> Style {
    Style::new().bg(GREEN).fg(Color::White).add_modifier(Modifier::BOLD)
}

fn block(title: String) -> Block<'static> {
    Block::new().borders(Borders::ALL).border_style(Style::new().fg(Color::DarkGray)).title(Span::styled(title, Style::new().fg(LIGHT).add_modifier(Modifier::BOLD)))
}

// the whole screen: tab bar, the tab, output if shown, footer, and any popup on top
pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let output = if app.show_output { (area.height / 3).clamp(4, 12) } else { 0 };
    let [bar, body, pane, footer] = Layout::vertical([Constraint::Length(1), Constraint::Min(3), Constraint::Length(output), Constraint::Length(1)]).areas(area);

    draw_bar(frame, app, bar);
    draw_tab(frame, app, body);
    if app.show_output {
        draw_output(frame, app, pane);
    }
    frame.render_widget(Paragraph::new(footer_line(app)), footer);
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

// numbered tabs, the current one green, recording where each landed for clicks
fn draw_bar(frame: &mut Frame, app: &App, area: Rect) {
    let label = |index: usize, name: &str| format!(" {} {name} ", index + 1);
    let full: u16 = TABS.iter().enumerate().map(|(index, tab)| label(index, tab.title()).len() as u16).sum();
    let name = |tab: Tab| if full <= area.width { tab.title() } else { short(tab) };

    let mut hits = app.hits.borrow_mut();
    hits.bar_y = area.y;
    hits.tabs.clear();
    let mut x = area.x;
    let spans: Vec<Span> = TABS
        .iter()
        .enumerate()
        .map(|(index, tab)| {
            let text = label(index, name(*tab));
            hits.tabs.push((x, x + text.len() as u16));
            x += text.len() as u16;
            if index == app.tab { Span::styled(text, selected()) } else { Span::raw(text) }
        })
        .collect();
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
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
            ListItem::new(cells.collect::<String>())
        })
        .collect();

    let list = List::new(items).block(block).highlight_style(selected());
    let mut list_state = ListState::default().with_selected((!rows.is_empty()).then_some(state.selected));
    frame.render_stateful_widget(list, area, &mut list_state);
    let mut hits = app.hits.borrow_mut();
    *hits = Hits { list: area, offset: list_state.offset(), ..hits.clone() };
}

// + green, - red, @@ cyan, like `maw diff` on a terminal
fn diff_line(line: &str) -> Line<'_> {
    let color = match line.chars().next() {
        Some('+') if !line.starts_with("+++") => Color::Green,
        Some('-') if !line.starts_with("---") => Color::Red,
        Some('@') => Color::Cyan,
        _ => Color::Reset,
    };
    Line::styled(line, Style::new().fg(color))
}

// the last lines the running or last command printed
fn draw_output(frame: &mut Frame, app: &App, area: Rect) {
    let title = match &app.busy {
        Some(label) => format!(" {label} "),
        None => " output ".into(),
    };
    let visible = area.height.saturating_sub(2) as usize;
    let lines = &app.output[app.output.len().saturating_sub(visible)..];
    frame.render_widget(Paragraph::new(lines.join("\n")).block(block(title)), area);
}

// "i install  r remove" with each key letter green
fn key_spans(keys: &str) -> Vec<Span<'_>> {
    keys.split("  ")
        .flat_map(|hint| {
            let (key, action) = hint.split_once(' ').unwrap_or((hint, ""));
            [Span::styled(key, Style::new().fg(LIGHT).add_modifier(Modifier::BOLD)), Span::raw(format!(" {action}  "))]
        })
        .collect()
}

// the filter being typed, else what's running and the tab's keys
fn footer_line(app: &App) -> Line<'_> {
    if app.filtering {
        return Line::from(format!("/{}", app.state().filter));
    }
    let busy = app.busy.as_ref().map(|label| Span::styled(format!("{label}  "), Style::new().fg(Color::Yellow)));
    let keys = key_spans(app.current_tab().keys()).into_iter().chain(key_spans("/ filter  ? keys  q quit"));
    Line::from(busy.into_iter().chain(keys).collect::<Vec<_>>())
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
    let block = Block::new().borders(Borders::ALL).border_style(Style::new().fg(GREEN)).title(Span::styled(format!(" {title} "), Style::new().fg(LIGHT).add_modifier(Modifier::BOLD)));
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
        assert_eq!(hits.tabs[0], (0, " 1 packages ".len() as u16));
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
    fn unloaded_tabs_say_so() {
        let app = App::new();
        assert!(screen(&app, 100, 20).contains("loading"));
    }
}
