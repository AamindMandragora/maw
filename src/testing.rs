use crate::env::Env;
use crate::repo::Repo;
use crate::runner::RunError;
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
    fs::create_dir_all(env.sysroot.join("var/db/xbps")).unwrap();

    let (repo, _) = Repo::init(&env, &FakeRunner::new(|_, _| String::new()), &dir.path().join("dots")).unwrap();
    modules.iter().for_each(|name| write(&repo.module_file(name), ""));
    Fixture { dir, env, repo, runner: FakeRunner::fallible(fake) }
}

// nix, plus xbps on a system where only bash is installed and the repo has bash, foot, and waybar
fn fake(program: &str, args: &[String]) -> Result<String, RunError> {
    if program != "xbps-query" {
        return Ok(fake_nix(args));
    }
    let failed = || RunError::Failed { program: program.into(), stderr: String::new() };
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [.., "-m"] => Ok("bash-5.2_1\n".into()),
        [.., "-l"] => Ok("ii bash-5.2_1  GNU shell\n".into()),
        [.., "-R", name] if ["bash", "foot", "waybar"].contains(name) => Ok(format!("pkgver: {name}-1.0_1\nshort_desc: {name}\n")),
        [.., "-R", _] => Err(failed()),
        _ => Ok(String::new()),
    }
}

// answers registry.nix, maw.nix, and -A modules.<name> like nix-instantiate would
fn fake_nix(args: &[String]) -> String {
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
        return json!({ "packages": { "xbps": declared_xbps(Path::new(target)) }, "paths": {} }).to_string();
    }
    let name = args[args.len() - 2].trim_start_matches("modules.");
    json!([{ "name": name, "key": "main", "content": format!("{name} v1\n"), "executable": false, "scope": "user" }]).to_string()
}

// the quoted names on maw.nix's `xbps = [ ... ];` line, which maw always writes on one line
fn declared_xbps(maw_file: &Path) -> Vec<String> {
    let text = fs::read_to_string(maw_file).unwrap_or_default();
    let line = text.lines().find(|line| line.trim_start().starts_with("xbps = [")).unwrap_or("");
    line.split('"').skip(1).step_by(2).map(String::from).collect()
}

pub fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}
