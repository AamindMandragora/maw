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

// nix; xbps with only bash installed and bash, foot, waybar in the repo; crates.io with bat and ripgrep; nothing from go
fn fake(program: &str, args: &[String]) -> Result<String, RunError> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match program {
        "xbps-query" => fake_xbps(program, &args),
        "cargo" => Ok(fake_cargo(&args)),
        "curl" => Ok(fake_crates_io(&args)),
        "go" | "flatpak" | "xbps-install" | "xbps-remove" | "xbps-rindex" | "xbps-pkgdb" | "sv" | "tail" => Ok(String::new()),
        "install" | "cp" | "ln" | "rm" | "mkdir" | "git" => real(program, &args),
        "date" => Ok("2026-09-23 12:00\n".into()),
        _ => Ok(fake_nix(&args)),
    }
}

// crates.io search results for the crates that exist: bat and ripgrep are programs, libfoo a library
fn fake_cargo(args: &[&str]) -> String {
    match args {
        ["search", .., name] if ["bat", "ripgrep", "libfoo"].contains(name) => format!("{name} = \"1.0.0\"    # {name} crate\n"),
        _ => String::new(),
    }
}

// the crates.io api's list of programs a crate version builds
fn fake_crates_io(args: &[&str]) -> String {
    let programs = if args.last().is_some_and(|url| url.contains("/libfoo/")) { "[]" } else { r#"["bin"]"# };
    format!(r#"{{"version":{{"bin_names":{programs}}}}}"#)
}

fn fake_xbps(program: &str, args: &[&str]) -> Result<String, RunError> {
    let failed = || RunError::Failed { program: program.into(), stderr: String::new() };
    match args {
        [.., "-m"] => Ok("bash-5.2_1\n".into()),
        [.., "-l"] => Ok("ii bash-5.2_1  GNU shell\n".into()),
        [.., "-R", name] if ["bash", "foot", "waybar"].contains(name) => Ok(format!("pkgver: {name}-1.0_1\nshort_desc: {name}\n")),
        [.., "-R", _] => Err(failed()),
        [.., "-Rs", term] if ["bash", "foot", "waybar"].contains(term) => Ok(format!("[-] {term}-1.0_1  {term}\n")),
        _ => Ok(String::new()),
    }
}

// answers registry.nix, maw.nix, and -A modules.<name> like nix-instantiate would
fn fake_nix(args: &[&str]) -> String {
    let target = args.last().unwrap();
    if target.ends_with("registry.nix") {
        return json!({
            "bash": { "format": "shell", "files": { "main": "~/.bashrc" } },
            "foot": { "format": "ini", "files": { "main": "foot/foot.ini" } },
            "scripts": { "dir": "~/.local/bin", "executable": true },
            "wallpapers": { "dir": "~/.local/share/wallpapers" },
            "greetd": { "format": "toml", "files": { "main": "/etc/greetd/config.toml" }, "root": true },
            "waybar": { "format": "json", "files": { "main": "waybar/config.jsonc", "style": "waybar/style.css" } },
        })
        .to_string();
    }
    if target.ends_with("maw.nix") {
        let lists = |keys: &[&str]| -> serde_json::Map<String, serde_json::Value> {
            keys.iter()
                .map(|key| (key.to_string(), json!(declared(Path::new(target), key))))
                .filter(|(_, names)| names.as_array().is_some_and(|names| !names.is_empty()))
                .collect()
        };
        return json!({ "packages": lists(&["xbps", "cargo", "go"]), "services": lists(&["system", "user"]), "paths": {} }).to_string();
    }
    if args[args.len() - 2] == "settings" {
        return "{}".into();
    }
    let name = args[args.len() - 2].trim_start_matches("modules.");

    // usersvc and syssvc are services whose run line carries the module's text, so editing the module changes them
    if let Some(scope) = [("usersvc", "user"), ("syssvc", "system")].iter().find_map(|(module, scope)| (name == *module).then_some(scope)) {
        let text = fs::read_to_string(Path::new(target).join("modules").join(format!("{name}.nix"))).unwrap_or_default();
        let service = json!({ "run": format!("exec sleep 1000 # {}", text.trim()), "log": true, "enable": true, "env": {} });
        return json!([{ "name": name, "key": "service", "content": "", "executable": false, "scope": scope, "service": service }]).to_string();
    }
    json!([{ "name": name, "key": "main", "content": format!("{name} v1\n"), "executable": false, "scope": "user" }]).to_string()
}

// the quoted names on maw.nix's `<backend> = [ ... ];` line, which maw always writes on one line
fn declared(maw_file: &Path, backend: &str) -> Vec<String> {
    let text = fs::read_to_string(maw_file).unwrap_or_default();
    let line = text.lines().find(|line| line.trim_start().starts_with(&format!("{backend} = ["))).unwrap_or("");
    line.split('"').skip(1).step_by(2).map(String::from).collect()
}

// file and git commands really run, confined to the fixture's tempdir, with a fixed git identity
fn real(program: &str, args: &[&str]) -> Result<String, RunError> {
    let identity = [("GIT_AUTHOR_NAME", "maw"), ("GIT_AUTHOR_EMAIL", "maw@test"), ("GIT_COMMITTER_NAME", "maw"), ("GIT_COMMITTER_EMAIL", "maw@test")];
    let output = std::process::Command::new(program).args(args).envs(identity).output().unwrap();
    match output.status.success() {
        true => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
        false => Err(RunError::Failed { program: program.into(), stderr: String::from_utf8_lossy(&output.stderr).into_owned() }),
    }
}

pub fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}
