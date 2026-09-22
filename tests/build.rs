use maw::build::build;
use maw::env::Env;
use maw::repo::Repo;
use maw::runner::SystemRunner;
use std::fs;
use std::path::Path;
use std::process::Command;

// builds a copy of the fixture dotfiles with real nix, into a tempdir home
#[test]
fn fixture_builds_to_goldens_then_no_ops() {
    let dir = tempfile::tempdir().unwrap();
    let dots = dir.path().join("dots");
    Command::new("cp").args(["-r", "tests/fixtures/dotfiles"]).arg(&dots).status().unwrap();

    let env = Env::new(&dir.path().join("home"), &dir.path().join("sys"), Path::new(env!("CARGO_MANIFEST_DIR")));
    let (repo, _) = Repo::init(&env, &SystemRunner, &dots).unwrap();

    let first = build(&env, &SystemRunner, &repo).unwrap();
    assert_eq!(first.evaluated, ["bash", "fuzzel", "niri", "waybar"]);

    // each out/ file matches its golden, and lands where the registry says
    let pairs = [("niri/config.kdl", "niri/main"), ("waybar/style.css", "waybar/style"), ("bash/.bashrc", "bash/main")];
    pairs.iter().for_each(|(out, golden)| {
        let rendered = fs::read_to_string(repo.out_dir().join(out)).unwrap();
        assert_eq!(rendered, fs::read_to_string(Path::new("tests/golden").join(golden)).unwrap(), "{out}");
    });
    let niri = first.outputs.iter().find(|output| output.name == "niri").unwrap();
    assert_eq!(niri.destination, env.home.join(".config/niri/config.kdl"));

    let second = build(&env, &SystemRunner, &repo).unwrap();
    assert!(second.evaluated.is_empty() && second.written.is_empty());
}
