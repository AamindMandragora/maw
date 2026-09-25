// the terminal ui: `maw` with no arguments
use crate::cli::{self, Command, Hooks};
use crate::env::Env;
use crate::repo::Repo;
use crate::runner::SystemRunner;
use anyhow::Result;
use app::{App, Request, TABS, Tab};
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;
use term::Term;

pub mod app;
pub mod data;
pub mod term;
pub mod view;

// what the background command and the output reader tell the event loop
enum Message {
    Line(String),
    Ask(String, Sender<Option<String>>),
    Edit(PathBuf, Sender<Result<()>>),
    Done(Result<(), String>),
}

// asks for the sudo password up front, takes over the terminal, and runs until q
pub fn run(env: &Env) -> Result<()> {
    Repo::locate(env)?;
    if env.sysroot == Path::new("/") {
        let _ = std::process::Command::new("sudo").arg("-v").status();
    }

    let (sender, receiver) = mpsc::channel();
    install_hooks(&sender);
    let (mut term, pipe) = Term::enter()?;
    spawn_reader(pipe, sender.clone());

    let result = event_loop(env, &mut term, &sender, &receiver);
    term.suspend()?;
    result
}

// questions become popups and the editor gets the terminal, both by asking the event loop and waiting
fn install_hooks(sender: &Sender<Message>) {
    let (ask_sender, edit_sender) = (Mutex::new(sender.clone()), Mutex::new(sender.clone()));
    cli::set_hooks(Hooks {
        ask: Box::new(move |question| {
            let (reply, answer) = mpsc::channel();
            ask_sender.lock().ok()?.send(Message::Ask(question.into(), reply)).ok()?;
            answer.recv().ok().flatten()
        }),
        edit: Box::new(move |path| {
            let (reply, done) = mpsc::channel();
            edit_sender.lock().map_err(|_| anyhow::anyhow!("tui gone"))?.send(Message::Edit(path.into(), reply))?;
            done.recv()?
        }),
    });
}

// turns what commands print into output lines; a carriage return (a progress bar) starts the line over
fn spawn_reader(pipe: impl Read + Send + 'static, sender: Sender<Message>) {
    std::thread::spawn(move || {
        BufReader::new(pipe).lines().map_while(Result::ok).for_each(|line| {
            let last = line.rsplit('\r').find(|part| !part.trim().is_empty()).unwrap_or("").to_string();
            let _ = sender.send(Message::Line(last));
        });
    });
}

fn event_loop(env: &Env, term: &mut Term, sender: &Sender<Message>, receiver: &Receiver<Message>) -> Result<()> {
    let runner = SystemRunner;
    let mut app = App::new();
    let mut reply: Option<Sender<Option<String>>> = None;
    let mut previewed = (usize::MAX, String::new());

    loop {
        term.draw(&app)?;

        // a tab loads the first time it's shown, and again after anything runs
        if !app.state().loaded && app.finding.is_none() {
            let tab = app.current_tab();
            app.set_rows(tab, data::load(env, &runner, tab));
            continue;
        }

        // the preview follows the selection
        let selected = (app.tab, app.state().current().map(|row| row.key.clone()).unwrap_or_default());
        if selected != previewed {
            app.preview = data::preview(env, app.current_tab(), &selected.1);
            previewed = selected;
        }

        let request = match event::poll(Duration::from_millis(100))? {
            true => match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key),
                Event::Mouse(mouse) => app.handle_mouse(mouse),
                _ => None,
            },
            false => None,
        };
        {
            match request {
                Some(Request::Quit) if app.busy.is_none() => return Ok(()),
                Some(Request::Quit) => app.output.push("wait for it to finish first".into()),
                Some(Request::Reload) => reload(&mut app),
                Some(Request::Find(term_text)) => {
                    let rows = data::find_rows(env, &runner, &term_text);
                    app.finding = Some(term_text);
                    app.set_rows(Tab::Packages, rows);
                }
                Some(Request::Answer(answer)) => {
                    if let Some(reply) = reply.take() {
                        let _ = reply.send(answer);
                    }
                }
                Some(Request::Run { command, label }) => start(env, term, &mut app, sender, command, label)?,
                None => {}
            }
        }

        // everything the background command sent since the last frame
        while let Ok(message) = receiver.try_recv() {
            match message {
                Message::Line(line) => app.output.push(line),
                Message::Ask(question, answer) => {
                    app.ask(&question);
                    reply = Some(answer);
                }
                Message::Edit(path, done) => {
                    term.suspend()?;
                    let result = edit(&path);
                    term.resume()?;
                    let _ = done.send(result);
                }
                Message::Done(result) => {
                    app.output.push(match result {
                        Ok(()) => format!("{} done", app.busy.take().unwrap_or_default()),
                        Err(error) => format!("error: {error}"),
                    });
                    app.busy = None;
                    reload(&mut app);
                }
            }
        }
    }
}

// every tab reloads the next time it's shown
fn reload(app: &mut App) {
    app.finding = None;
    TABS.iter().enumerate().for_each(|(index, _)| app.tabs[index].loaded = false);
}

// runs a command in the background with the cli's own code, after making sure sudo won't need to prompt
fn start(env: &Env, term: &mut Term, app: &mut App, sender: &Sender<Message>, command: Command, label: String) -> Result<()> {
    ensure_sudo(env, term)?;
    app.output = vec![format!("maw {label}")];
    app.show_output = true;
    app.busy = Some(label);

    let (env, sender) = (env.clone(), sender.clone());
    std::thread::spawn(move || {
        let result = cli::run(&env, &SystemRunner, command).map_err(|error| format!("{error:#}"));
        let _ = sender.send(Message::Done(result));
    });
    Ok(())
}

// root steps run through sudo without a terminal, so an expired timestamp is renewed first, outside the tui
fn ensure_sudo(env: &Env, term: &mut Term) -> Result<()> {
    if env.sysroot != Path::new("/") {
        return Ok(());
    }
    let valid = std::process::Command::new("sudo").args(["-n", "true"]).status().is_ok_and(|status| status.success());
    if !valid {
        term.suspend()?;
        let _ = std::process::Command::new("sudo").arg("-v").status();
        term.resume()?;
    }
    Ok(())
}

// the editor on the real terminal
fn edit(path: &Path) -> Result<()> {
    let editor = cli::editor();
    let mut words = editor.split_whitespace();
    let program = words.next().unwrap_or("vi");
    let status = std::process::Command::new(program).args(words).arg(path).status()?;
    anyhow::ensure!(status.success(), "{program} exited with {status}");
    Ok(())
}
