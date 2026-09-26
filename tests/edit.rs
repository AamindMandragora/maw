use maw::activate::{Ask, Options, activate};
use maw::edit::{add, new_module};
use maw::env::Env;
use maw::repo::Repo;
use maw::runner::SystemRunner;
use maw::status::{diff, status};
use std::fs;
use std::path::Path;

struct Nobody;

impl Ask for Nobody {
    fn ask(&self, _: &str) -> Option<String> {
        None
    }
}

// an empty repo in a tempdir home, evaluated with real nix
fn setup() -> (tempfile::TempDir, Env, Repo) {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::new(&dir.path().join("home"), &dir.path().join("sys"), Path::new(env!("CARGO_MANIFEST_DIR")));
    let (repo, _) = Repo::init(&env, &SystemRunner, &dir.path().join("dots")).unwrap();
    (dir, env, repo)
}

fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

// live configs imported by `new` render back byte for byte, so activating changes nothing but the link
#[test]
fn imported_configs_render_unchanged() {
    let (_dir, env, repo) = setup();
    let foot = "[main]\nfont=${FONT}\nshell=''quoted''\n\n\tindented=tab\n";
    let style = "window { color: red; }";
    write(&env.home.join(".config/foot/foot.ini"), foot);
    write(&env.home.join(".config/waybar/style.css"), style);

    new_module(&env, &SystemRunner, &repo, "foot", None).unwrap();
    new_module(&env, &SystemRunner, &repo, "waybar", None).unwrap();
    activate(&env, &SystemRunner, &repo, &Nobody, Options::default()).unwrap();

    assert_eq!(fs::read_to_string(env.home.join(".config/foot/foot.ini")).unwrap(), foot);
    assert_eq!(fs::read_to_string(env.home.join(".config/waybar/style.css")).unwrap(), style);
    assert!(fs::symlink_metadata(env.home.join(".config/foot/foot.ini")).unwrap().is_symlink());
}

// added files come back as links to static/, wherever they lived
#[test]
fn added_files_link_back_in_place() {
    let (_dir, env, repo) = setup();
    write(&env.home.join(".config/nvim/init.lua"), "vim.o.number = true\n");
    write(&env.home.join("notes.txt"), "hi\n");
    write(&env.home.join(".tmux.conf"), "set -g mouse on\n");

    add(&env, &SystemRunner, &repo, &env.home.join(".config/nvim"), None, None).unwrap();
    add(&env, &SystemRunner, &repo, &env.home.join("notes.txt"), None, None).unwrap();
    add(&env, &SystemRunner, &repo, &env.home.join(".tmux.conf"), Some("tmux"), None).unwrap();
    // same content, but each live file is a real file in the way of its link
    assert!(diff(&env, &SystemRunner, &repo).unwrap().is_empty());
    assert_eq!(status(&env, &SystemRunner, &repo).unwrap().len(), 3);
    activate(&env, &SystemRunner, &repo, &Nobody, Options::default()).unwrap();

    [".config/nvim/init.lua", "notes.txt", ".tmux.conf"].iter().for_each(|file| {
        let target = fs::read_link(env.home.join(file)).unwrap_or_else(|_| panic!("{file} not linked"));
        assert!(target.starts_with(repo.static_dir()), "{file}");
    });
    assert!(diff(&env, &SystemRunner, &repo).unwrap().is_empty());
    assert!(status(&env, &SystemRunner, &repo).unwrap().is_empty());
}
