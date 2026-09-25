use crate::cli::{Command, ServiceName, SvAction};
use crate::init::Scope;
use crate::style::Tone;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::cell::RefCell;

// the tabs, in the order 1 through 7 select them
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Packages,
    Modules,
    Services,
    Status,
    Diff,
    History,
    Git,
}

pub const TABS: [Tab; 7] = [Tab::Packages, Tab::Modules, Tab::Services, Tab::Status, Tab::Diff, Tab::History, Tab::Git];

impl Tab {
    pub fn title(self) -> &'static str {
        match self {
            Tab::Packages => "packages",
            Tab::Modules => "modules",
            Tab::Services => "services",
            Tab::Status => "status",
            Tab::Diff => "diff",
            Tab::History => "history",
            Tab::Git => "git",
        }
    }

    // the action keys shown in the footer
    pub fn keys(self) -> &'static str {
        match self {
            Tab::Packages => "i install  r remove  f find  s sync  A adopt",
            Tab::Modules => "e edit  n new  a add file",
            Tab::Services => "e enable  d disable  r restart  s status",
            Tab::Status => "a activate  F activate --force",
            Tab::Diff => "a activate",
            Tab::History => "r roll back to this generation",
            Tab::Git => "c commit  p push  P pull",
        }
    }

    fn index(self) -> usize {
        TABS.iter().position(|tab| *tab == self).unwrap()
    }
}

// one line of a tab: its columns, the key actions use (a package target, a module name, a generation number), and its tone
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub cells: Vec<String>,
    pub key: String,
    pub tone: Option<Tone>,
}

impl Row {
    pub fn new(cells: Vec<String>, key: impl Into<String>) -> Self {
        Row { cells, key: key.into(), tone: None }
    }

    // a plain line of text with no key
    pub fn text(line: impl Into<String>) -> Self {
        Row { cells: vec![line.into()], key: String::new(), tone: None }
    }

    pub fn toned(self, tone: Option<Tone>) -> Self {
        Row { tone, ..self }
    }
}

#[derive(Debug, Default)]
pub struct TabState {
    pub rows: Vec<Row>,
    pub selected: usize,
    pub filter: String,
    pub loaded: bool,
}

impl TabState {
    // rows matching the filter, case-insensitively, anywhere in their columns
    pub fn visible(&self) -> Vec<&Row> {
        let filter = self.filter.to_lowercase();
        self.rows.iter().filter(|row| filter.is_empty() || row.cells.join(" ").to_lowercase().contains(&filter)).collect()
    }

    pub fn current(&self) -> Option<&Row> {
        self.visible().get(self.selected).copied()
    }
}

// what a text popup's answer becomes
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputFor {
    Install,
    Find,
    NewModule,
    AddFile,
    CommitMessage,
    // an answer to a question the running command asked
    Answer,
}

#[derive(Debug, Clone)]
pub enum Popup {
    Input { title: String, input: String, then: InputFor },
    Confirm { question: String, command: Command, label: String },
    Help,
}

// what the app asks the event loop to do
#[derive(Debug, Clone)]
pub enum Request {
    Run { command: Command, label: String },
    Find(String),
    Answer(Option<String>),
    Reload,
    Quit,
}

// where the last frame drew clickable things: each tab as (row, first column, end column), and the list with its scroll
#[derive(Debug, Default, Clone)]
pub struct Hits {
    pub tabs: Vec<(u16, u16, u16)>,
    pub list: Rect,
    pub offset: usize,
}

#[derive(Debug, Default)]
pub struct App {
    // written by the view as it draws, read by mouse handling
    pub hits: RefCell<Hits>,
    pub tab: usize,
    pub tabs: Vec<TabState>,
    pub filtering: bool,
    pub popup: Option<Popup>,
    pub output: Vec<String>,
    pub show_output: bool,
    pub preview: Vec<String>,
    // the label of the command running in the background, if any
    pub busy: Option<String>,
    // packages tab is showing repo search results for this term
    pub finding: Option<String>,
}

impl App {
    pub fn new() -> Self {
        App { tabs: TABS.iter().map(|_| TabState::default()).collect(), ..App::default() }
    }

    pub fn current_tab(&self) -> Tab {
        TABS[self.tab]
    }

    pub fn state(&self) -> &TabState {
        &self.tabs[self.tab]
    }

    fn state_mut(&mut self) -> &mut TabState {
        &mut self.tabs[self.tab]
    }

    // replaces a tab's rows, keeping the selection in range
    pub fn set_rows(&mut self, tab: Tab, rows: Vec<Row>) {
        let state = &mut self.tabs[tab.index()];
        state.rows = rows;
        state.loaded = true;
        state.selected = state.selected.min(state.visible().len().saturating_sub(1));
    }

    // a question from the running command, answered in a popup
    pub fn ask(&mut self, question: &str) {
        self.popup = Some(Popup::Input { title: question.trim().to_string(), input: String::new(), then: InputFor::Answer });
    }

    // routes a key to the popup, the filter, or the tab
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Request> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Some(Request::Quit);
        }
        if self.popup.is_some() {
            return self.popup_key(key);
        }
        if self.filtering {
            return self.filter_key(key);
        }
        match self.navigate(key) {
            Some(request) => request,
            None if self.busy.is_some() => None,
            None => self.action(key),
        }
    }

    // clicks pick a tab or a row; the wheel moves the selection, or scrolls the diff
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> Option<Request> {
        if self.popup.is_some() {
            return None;
        }
        let last = self.state().visible().len().saturating_sub(1);
        let hits = self.hits.borrow().clone();
        let is_list = self.current_tab() != Tab::Diff;
        let selected = &mut self.tabs[self.tab].selected;
        match mouse.kind {
            MouseEventKind::ScrollDown => *selected = (*selected + 1).min(last),
            MouseEventKind::ScrollUp => *selected = selected.saturating_sub(1),
            MouseEventKind::Down(MouseButton::Left) if hits.tabs.iter().any(|(row, _, _)| *row == mouse.row) => {
                if let Some(index) = hits.tabs.iter().position(|(row, start, end)| *row == mouse.row && (*start..*end).contains(&mouse.column)) {
                    self.tab = index;
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                // rows start one line in, below the border
                let inside = mouse.row > hits.list.y && mouse.row + 1 < hits.list.y + hits.list.height && (hits.list.x..hits.list.x + hits.list.width).contains(&mouse.column);
                let index = hits.offset + mouse.row.saturating_sub(hits.list.y + 1) as usize;
                if inside && index <= last && is_list {
                    *selected = index;
                }
            }
            _ => {}
        }
        None
    }

    // text popups take typing; confirms take y or n; any key closes help
    fn popup_key(&mut self, key: KeyEvent) -> Option<Request> {
        let popup = self.popup.take()?;
        match (popup, key.code) {
            (Popup::Input { then, .. }, KeyCode::Esc) => (then == InputFor::Answer).then_some(Request::Answer(None)),
            (Popup::Input { input, then, .. }, KeyCode::Enter) => self.submit(then, input.trim()),
            (Popup::Input { title, mut input, then }, KeyCode::Backspace) => {
                input.pop();
                self.popup = Some(Popup::Input { title, input, then });
                None
            }
            (Popup::Input { title, mut input, then }, KeyCode::Char(char)) => {
                input.push(char);
                self.popup = Some(Popup::Input { title, input, then });
                None
            }
            (Popup::Confirm { command, label, .. }, KeyCode::Char('y') | KeyCode::Enter) => Some(Request::Run { command, label }),
            (popup @ Popup::Input { .. } | popup @ Popup::Confirm { .. }, code) if !matches!(code, KeyCode::Esc | KeyCode::Char('n')) => {
                self.popup = Some(popup);
                None
            }
            _ => None,
        }
    }

    // what an entered line turns into
    fn submit(&mut self, then: InputFor, text: &str) -> Option<Request> {
        if then == InputFor::Answer {
            return Some(Request::Answer(Some(text.into())));
        }
        if text.is_empty() {
            return None;
        }
        let run = |command: Command, label: String| Some(Request::Run { command, label });
        match then {
            InputFor::Install => run(Command::Install { packages: text.split_whitespace().map(String::from).collect(), dry_run: false }, format!("install {text}")),
            InputFor::Find => Some(Request::Find(text.into())),
            InputFor::NewModule => run(Command::New { name: text.into(), format: None, no_activate: false }, format!("new {text}")),
            InputFor::AddFile => run(Command::Add { path: text.into(), name: None, no_activate: false }, format!("add {text}")),
            InputFor::CommitMessage => run(Command::Commit { message: Some(text.into()) }, "commit".into()),
            InputFor::Answer => None,
        }
    }

    // typing narrows the list; enter keeps the filter, esc drops it
    fn filter_key(&mut self, key: KeyEvent) -> Option<Request> {
        match key.code {
            KeyCode::Esc => {
                self.filtering = false;
                self.state_mut().filter.clear();
            }
            KeyCode::Enter => self.filtering = false,
            KeyCode::Backspace => {
                self.state_mut().filter.pop();
            }
            KeyCode::Char(char) => self.state_mut().filter.push(char),
            _ => {}
        }
        self.state_mut().selected = 0;
        None
    }

    // keys that work on every tab, even while a command runs; None when the key isn't one of them
    fn navigate(&mut self, key: KeyEvent) -> Option<Option<Request>> {
        let last = self.state().visible().len().saturating_sub(1);
        let selected = &mut self.tabs[self.tab].selected;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => *selected = (*selected + 1).min(last),
            KeyCode::Char('k') | KeyCode::Up => *selected = selected.saturating_sub(1),
            KeyCode::Char('g') | KeyCode::Home => *selected = 0,
            KeyCode::Char('G') | KeyCode::End => *selected = last,
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => self.tab = (self.tab + 1) % TABS.len(),
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => self.tab = (self.tab + TABS.len() - 1) % TABS.len(),
            KeyCode::Char(digit @ '1'..='7') => self.tab = digit as usize - '1' as usize,
            KeyCode::Char('/') => self.filtering = true,
            KeyCode::Char('o') => self.show_output = !self.show_output,
            KeyCode::Char('?') => self.popup = Some(Popup::Help),
            KeyCode::Char('R') => return Some(Some(Request::Reload)),
            KeyCode::Char('q') => return Some(Some(Request::Quit)),
            KeyCode::Esc if self.finding.is_some() => {
                self.finding = None;
                return Some(Some(Request::Reload));
            }
            _ => return None,
        }
        Some(None)
    }

    // the current tab's own keys, as commands (some asking first)
    fn action(&mut self, key: KeyEvent) -> Option<Request> {
        let row_key = self.state().current().map(|row| row.key.clone()).unwrap_or_default();
        let input = |title: &str, then: InputFor| Popup::Input { title: title.into(), input: String::new(), then };
        let confirm = |question: String, command: Command, label: String| Popup::Confirm { question, command, label };
        let run = |command: Command, label: String| Some(Request::Run { command, label });
        let service = || service_name(&row_key);

        match (self.current_tab(), key.code) {
            (Tab::Packages, KeyCode::Char('i')) => self.popup = Some(input("install", InputFor::Install)),
            (Tab::Packages, KeyCode::Enter) if self.finding.is_some() && !row_key.is_empty() => {
                return run(Command::Install { packages: vec![row_key.clone()], dry_run: false }, format!("install {row_key}"));
            }
            (Tab::Packages, KeyCode::Char('f')) => self.popup = Some(input("find in the repos", InputFor::Find)),
            (Tab::Packages, KeyCode::Char('r')) if !row_key.is_empty() => {
                self.popup = Some(confirm(format!("remove {row_key}?"), Command::Remove { packages: vec![row_key.clone()], dry_run: false }, format!("remove {row_key}")));
            }
            (Tab::Packages, KeyCode::Char('s')) => return run(Command::Sync { release: false }, "sync".into()),
            (Tab::Packages, KeyCode::Char('A')) => return run(Command::Adopt { dry_run: false }, "adopt".into()),
            (Tab::Modules, KeyCode::Char('e') | KeyCode::Enter) if !row_key.is_empty() => {
                return run(Command::Edit { name: row_key.clone(), no_activate: false }, format!("edit {row_key}"));
            }
            (Tab::Modules, KeyCode::Char('n')) => self.popup = Some(input("new module for", InputFor::NewModule)),
            (Tab::Modules, KeyCode::Char('a')) => self.popup = Some(input("add file or dir", InputFor::AddFile)),
            (Tab::Services, KeyCode::Char('e')) if !row_key.is_empty() => return run(Command::Sv { action: SvAction::Enable(service()) }, format!("enable {}", service().name)),
            (Tab::Services, KeyCode::Char('d')) if !row_key.is_empty() => {
                let name = service().name;
                self.popup = Some(confirm(format!("disable {name}?"), Command::Sv { action: SvAction::Disable(service()) }, format!("disable {name}")));
            }
            (Tab::Services, KeyCode::Char('r')) if !row_key.is_empty() => return run(Command::Sv { action: SvAction::Restart(service()) }, format!("restart {}", service().name)),
            (Tab::Services, KeyCode::Char('s')) if !row_key.is_empty() => return run(Command::Sv { action: SvAction::Status(service()) }, format!("status {}", service().name)),
            (Tab::Status | Tab::Diff, KeyCode::Char('a')) => return run(activate(false), "activate".into()),
            (Tab::Status, KeyCode::Char('F')) => self.popup = Some(confirm("activate --force, replacing drifted files?".into(), activate(true), "activate --force".into())),
            (Tab::History, KeyCode::Char('r') | KeyCode::Enter) if !row_key.is_empty() => {
                let command = Command::Rollback { generation: row_key.parse().ok(), dry_run: false };
                self.popup = Some(confirm(format!("roll back to generation {row_key}?"), command, format!("rollback to {row_key}")));
            }
            (Tab::Git, KeyCode::Char('c')) => self.popup = Some(input("commit message", InputFor::CommitMessage)),
            (Tab::Git, KeyCode::Char('p')) => return run(Command::Push, "push".into()),
            (Tab::Git, KeyCode::Char('P')) => return run(Command::Pull, "pull".into()),
            _ => {}
        }
        None
    }
}

fn activate(force: bool) -> Command {
    Command::Activate { dry_run: false, force, no_commit: false, no_build: false }
}

// a services row's key is "<scope>:<name>"
fn service_name(key: &str) -> ServiceName {
    let (scope, name) = key.split_once(':').unwrap_or(("system", key));
    let user = scope == Scope::User.to_string();
    ServiceName { name: name.into(), user, system: !user }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(app: &mut App, code: KeyCode) -> Option<Request> {
        app.handle_key(KeyEvent::from(code))
    }

    fn typed(app: &mut App, text: &str) {
        text.chars().for_each(|char| {
            press(app, KeyCode::Char(char));
        });
    }

    fn rows(keys: &[&str]) -> Vec<Row> {
        keys.iter().map(|key| Row::new(vec![key.to_string()], *key)).collect()
    }

    #[test]
    fn digits_and_tab_switch_tabs() {
        let mut app = App::new();
        press(&mut app, KeyCode::Char('3'));
        assert_eq!(app.current_tab(), Tab::Services);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.current_tab(), Tab::Status);
        (0..4).for_each(|_| {
            press(&mut app, KeyCode::BackTab);
        });
        assert_eq!(app.current_tab(), Tab::Git);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.current_tab(), Tab::Packages);
        press(&mut app, KeyCode::Left);
        assert_eq!(app.current_tab(), Tab::Git);
    }

    #[test]
    fn selection_stays_in_range() {
        let mut app = App::new();
        app.set_rows(Tab::Packages, rows(&["a", "b", "c"]));
        press(&mut app, KeyCode::Char('G'));
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.state().current().unwrap().key, "c");
        press(&mut app, KeyCode::Char('g'));
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.state().current().unwrap().key, "a");
    }

    #[test]
    fn filter_narrows_and_esc_clears_it() {
        let mut app = App::new();
        app.set_rows(Tab::Packages, rows(&["foot", "fuzzel", "niri"]));
        press(&mut app, KeyCode::Char('/'));
        typed(&mut app, "fu");
        assert_eq!(app.state().visible().len(), 1);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.state().visible().len(), 3);
    }

    #[test]
    fn install_asks_for_names_then_runs() {
        let mut app = App::new();
        press(&mut app, KeyCode::Char('i'));
        typed(&mut app, "foot cargo:bat");
        match press(&mut app, KeyCode::Enter) {
            Some(Request::Run { command: Command::Install { packages, .. }, .. }) => assert_eq!(packages, ["foot", "cargo:bat"]),
            other => panic!("{other:?}"),
        }
        assert!(app.popup.is_none());
    }

    #[test]
    fn removing_confirms_first() {
        let mut app = App::new();
        app.set_rows(Tab::Packages, rows(&["foot"]));
        assert!(press(&mut app, KeyCode::Char('r')).is_none());
        assert!(press(&mut app, KeyCode::Char('n')).is_none());
        assert!(app.popup.is_none());

        press(&mut app, KeyCode::Char('r'));
        assert!(matches!(press(&mut app, KeyCode::Char('y')), Some(Request::Run { command: Command::Remove { .. }, .. })));
    }

    #[test]
    fn service_keys_carry_their_scope() {
        let mut app = App::new();
        press(&mut app, KeyCode::Char('3'));
        app.set_rows(Tab::Services, vec![Row::new(vec!["rclone".into()], "user:rclone")]);
        match press(&mut app, KeyCode::Char('r')) {
            Some(Request::Run { command: Command::Sv { action: SvAction::Restart(service) }, .. }) => assert!(service.user && service.name == "rclone"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn questions_from_a_command_are_answered_or_dismissed() {
        let mut app = App::new();
        app.ask("activate now? [Y/n] ");
        typed(&mut app, "y");
        assert!(matches!(press(&mut app, KeyCode::Enter), Some(Request::Answer(Some(answer))) if answer == "y"));
        app.ask("commit message: ");
        assert!(matches!(press(&mut app, KeyCode::Esc), Some(Request::Answer(None))));
    }

    #[test]
    fn clicks_pick_tabs_and_rows_and_the_wheel_scrolls() {
        let mut app = App::new();
        app.set_rows(Tab::Packages, rows(&["a", "b", "c", "d"]));
        *app.hits.borrow_mut() = Hits { tabs: vec![(0, 0, 10), (0, 11, 20)], list: Rect::new(0, 1, 40, 10), offset: 1 };
        let click = |column, row| MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column, row, modifiers: KeyModifiers::NONE };

        app.handle_mouse(click(5, 3));
        assert_eq!(app.state().current().unwrap().key, "c");
        app.handle_mouse(MouseEvent { kind: MouseEventKind::ScrollDown, ..click(0, 0) });
        assert_eq!(app.state().current().unwrap().key, "d");
        app.handle_mouse(click(15, 0));
        assert_eq!(app.current_tab(), Tab::Modules);
    }

    #[test]
    fn nothing_new_starts_while_busy() {
        let mut app = App::new();
        app.busy = Some("sync".into());
        assert!(press(&mut app, KeyCode::Char('s')).is_none());
        press(&mut app, KeyCode::Char('2'));
        assert_eq!(app.current_tab(), Tab::Modules);
    }
}
