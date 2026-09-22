use maw::activate::{Ask, Options, Step, activate};
use maw::env::Env;
use maw::repo::Repo;
use maw::runner::SystemRunner;
use std::fs;
use std::path::Path;
use std::process::Command;

struct Answer(&'static str);

impl Ask for Answer {
    fn ask(&self, _: &str) -> Option<String> {
        Some(self.0.into())
    }
}

// activates a copy of the fixture with real nix: links land, a recorded answer places a loose file, a rerun does nothing
#[test]
fn fixture_activates_then_no_ops() {
    let dir = tempfile::tempdir().unwrap();
    let dots = dir.path().join("dots");
    Command::new("cp").args(["-r", "tests/fixtures/dotfiles"]).arg(&dots).status().unwrap();

    let env = Env::new(&dir.path().join("home"), &dir.path().join("sys"), Path::new(env!("CARGO_MANIFEST_DIR")));
    let (repo, _) = Repo::init(&env, &SystemRunner, &dots).unwrap();
    fs::write(repo.static_dir().join("notes.txt"), "hi\n").unwrap();

    let first = activate(&env, &SystemRunner, &repo, &Answer("~/notes.txt"), Options::default()).unwrap();
    assert_eq!(first.answered, ["notes.txt"]);
    assert_eq!(fs::read_link(env.home.join(".config/niri/config.kdl")).unwrap(), repo.out_dir().join("niri/config.kdl"));
    assert_eq!(fs::read_to_string(env.home.join("notes.txt")).unwrap(), "hi\n");

    let second = activate(&env, &SystemRunner, &repo, &Answer(""), Options::default()).unwrap();
    assert!(second.build.is_empty(), "{:?}", second.build);
    assert_eq!(second.steps, Vec::<Step>::new());
}
