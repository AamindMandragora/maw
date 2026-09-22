use crate::env::Env;
use crate::repo::Repo;
use crate::runner::fake::FakeRunner;
use serde_json::json;
use std::fs;
use std::path::Path;

// a scaffolded repo in a tempdir, with nix faked
pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub env: Env,
    pub repo: Repo,
    pub runner: FakeRunner,
}

// a repo whose modules/<name>.nix each "evaluate" to one file, "<name> v1"
pub fn fixture(modules: &[&str]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::new(&dir.path().join("home"), &dir.path().join("sys"), &dir.path().join("share"));
    ["lib.nix", "default.nix"].iter().for_each(|file| write(&env.nix_dir().join(file), ""));
    write(&env.registry_file(), "");

    let (repo, _) = Repo::init(&env, &FakeRunner::new(|_, _| String::new()), &dir.path().join("dots")).unwrap();
    modules.iter().for_each(|name| write(&repo.module_file(name), ""));
    Fixture { dir, env, repo, runner: FakeRunner::new(fake_nix) }
}

// answers registry.nix, maw.nix, and -A modules.<name> like nix-instantiate would
fn fake_nix(_: &str, args: &[String]) -> String {
    let target = args.last().unwrap();
    if target.ends_with("registry.nix") {
        return json!({
            "bash": { "format": "shell", "files": { "main": "~/.bashrc" } },
            "foot": { "format": "ini", "files": { "main": "foot/foot.ini" } },
            "scripts": { "dir": "~/.local/bin", "executable": true },
            "wallpapers": { "dir": "~/.local/share/wallpapers" },
            "waybar": { "format": "json", "files": { "main": "waybar/config.jsonc", "style": "waybar/style.css" } },
        })
        .to_string();
    }
    if target.ends_with("maw.nix") {
        return json!({ "paths": {} }).to_string();
    }
    let name = args[args.len() - 2].trim_start_matches("modules.");
    json!([{ "name": name, "key": "main", "content": format!("{name} v1\n"), "executable": false, "scope": "user" }]).to_string()
}

pub fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}
