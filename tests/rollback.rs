mod common;

use common::Contained;
use maw::env::Env;
use maw::repo::Repo;
use maw::rollback::restore;
use std::fs;
use std::path::Path;
use std::process::Command;

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git").arg("-C").arg(repo).args(args).envs([("GIT_AUTHOR_NAME", "t"), ("GIT_AUTHOR_EMAIL", "t@t"), ("GIT_COMMITTER_NAME", "t"), ("GIT_COMMITTER_EMAIL", "t@t")]).output().unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

// maw.nix with one xbps list for each machine
fn maw_nix(laptop: &str, desktop: &str) -> String {
    format!("{{\n  packages = {{ }};\n  services = {{ }};\n  paths = {{ }};\n  hosts = {{\n    desktop = {{\n      packages = {{\n        xbps = [ {desktop} ];\n      }};\n      services = {{ }};\n    }};\n    laptop = {{\n      packages = {{\n        xbps = [ {laptop} ];\n      }};\n      services = {{ }};\n    }};\n  }};\n}}\n")
}

// a rollback on the laptop takes back the laptop's changes and the shared ones, never the desktop's
#[test]
fn a_rollback_leaves_other_machines_alone() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env { host: "laptop".into(), ..Env::new(&dir.path().join("home"), &dir.path().join("sys"), Path::new(env!("CARGO_MANIFEST_DIR"))) };
    let (repo, _) = Repo::init(&env, &Contained, &dir.path().join("dots")).unwrap();
    let root = repo.root.clone();

    write(&repo.maw_file(), &maw_nix(r#""a""#, r#""b""#));
    write(&root.join("out/laptop/foot/foot.ini"), "old\n");
    write(&root.join("static/notes.txt"), "old\n");
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "generation 1"]);
    let first = git(&root, &["rev-parse", "HEAD"]);

    // both machines change things, and so does something shared
    write(&repo.maw_file(), &maw_nix(r#""a" "c""#, r#""b" "d""#));
    write(&root.join("out/laptop/foot/foot.ini"), "new\n");
    write(&root.join("out/desktop/foot/foot.ini"), "desktop\n");
    write(&root.join("hosts/desktop.pub"), "ssh-ed25519 desktop\n");
    write(&root.join("hosts/laptop.nix"), "{ scale = 2; }\n");
    write(&root.join("static/notes.txt"), "new\n");
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "generation 2"]);

    restore(&env, &Contained, &repo, &first).unwrap();
    let text = fs::read_to_string(repo.maw_file()).unwrap();
    assert!(text.contains("laptop = {\n      packages = {\n        xbps = [ \"a\" ];"), "{text}");
    assert!(text.contains("desktop = {\n      packages = {\n        xbps = [ \"b\" \"d\" ];"), "{text}");
    assert_eq!(fs::read_to_string(root.join("out/laptop/foot/foot.ini")).unwrap(), "old\n");
    assert_eq!(fs::read_to_string(root.join("static/notes.txt")).unwrap(), "old\n");
    assert!(!root.join("hosts/laptop.nix").exists());
    assert_eq!(fs::read_to_string(root.join("out/desktop/foot/foot.ini")).unwrap(), "desktop\n");
    assert!(root.join("hosts/desktop.pub").exists());
}
