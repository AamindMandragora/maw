use crate::activate::Wanted;
use crate::build::{Output, Report, ServiceRef, Settings};
use crate::env::Env;
use crate::inputs::{InputsError, hash_bytes, load_json, save_json};
use crate::init::Scope;
use crate::registry;
use crate::repo::Repo;
use crate::state::MawState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("no out/.maw/index.json in {0}; activate once with nix installed")]
    Missing(String),
    #[error(transparent)]
    Inputs(#[from] InputsError),
    #[error("{path}")]
    Io { path: PathBuf, source: std::io::Error },
}

// everything activation needs without nix: each file's source and destination, plus maw.nix and settings
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Index {
    pub files: Vec<Entry>,
    pub state: MawState,
    pub auto_commit: bool,
    pub void_packages: Option<String>,
}

// one file: its source relative to the repo, and its destination as ~/... or /... so it holds on any machine
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub source: PathBuf,
    pub destination: String,
    pub root: bool,
    pub service: Option<(String, Scope, bool)>,
}

// a destination as it reads on any machine: under home as ~/..., anything else relative to the system root
fn portable(env: &Env, path: &Path) -> String {
    match path.strip_prefix(&env.home) {
        Ok(relative) => format!("~/{}", relative.display()),
        Err(_) => format!("/{}", path.strip_prefix(&env.sysroot).or_else(|_| path.strip_prefix("/")).unwrap_or(path).display()),
    }
}

// records what an activation placed, for a later --no-build activation
pub fn write(env: &Env, repo: &Repo, build: &Report, wanted: &[Wanted]) -> Result<(), IndexError> {
    let services: HashMap<&PathBuf, &ServiceRef> = build.outputs.iter().filter_map(|output| Some((&output.out, output.service.as_ref()?))).collect();
    let files = wanted
        .iter()
        .map(|file| Entry {
            source: file.source.strip_prefix(&repo.root).unwrap_or(&file.source).to_path_buf(),
            destination: portable(env, &file.destination),
            root: file.root,
            service: services.get(&file.source).map(|service| (service.name.clone(), service.scope, service.enable)),
        })
        .collect();
    let index = Index { files, state: build.state.clone(), auto_commit: build.settings.auto_commit, void_packages: build.settings.void_packages.clone() };

    // unchanged activations leave it untouched, so a second activate writes nothing
    let path = crate::build::index_file(repo);
    if load_json::<Index>(&path).ok().as_ref() == Some(&index) {
        return Ok(());
    }
    Ok(save_json(&path, &index)?)
}

// a build report and static files rebuilt from the index, with no nix involved
pub fn load(env: &Env, repo: &Repo) -> Result<(Report, Vec<Wanted>), IndexError> {
    let path = crate::build::index_file(repo);
    if !path.exists() {
        return Err(IndexError::Missing(repo.root.display().to_string()));
    }
    let index: Index = load_json(&path)?;

    // rendered files come back as outputs, so services and diffs work as usual; static files as they are
    let (rendered, statics): (Vec<&Entry>, Vec<&Entry>) = index.files.iter().partition(|entry| entry.source.starts_with("out"));
    let outputs = rendered.into_iter().map(|entry| output(env, repo, entry)).collect::<Result<Vec<_>, _>>()?;
    let statics = statics
        .into_iter()
        .map(|entry| {
            let source = repo.root.join(&entry.source);
            let bytes = fs::read(&source).map_err(|error| IndexError::Io { path: source.clone(), source: error })?;
            Ok(Wanted { destination: registry::resolve(env, &entry.destination), hash: hash_bytes(&bytes), source, root: entry.root })
        })
        .collect::<Result<Vec<_>, IndexError>>()?;

    let settings = Settings { auto_commit: index.auto_commit, void_packages: index.void_packages, ..Settings::default() };
    Ok((Report { outputs, state: index.state, settings, ..Report::default() }, statics))
}

// one rendered file as an output, read back from out/
fn output(env: &Env, repo: &Repo, entry: &Entry) -> Result<Output, IndexError> {
    let out = repo.root.join(&entry.source);
    let content = fs::read_to_string(&out).map_err(|source| IndexError::Io { path: out.clone(), source })?;
    let mode = fs::metadata(&out).map_err(|source| IndexError::Io { path: out.clone(), source })?.permissions().mode();

    // out/<name>/... names the program; a service is named by its ref
    let folder = entry.source.components().nth(1).map(|part| part.as_os_str().to_string_lossy().into_owned()).unwrap_or_default();
    let name = entry.service.as_ref().map_or(folder, |(name, _, _)| name.clone());
    Ok(Output {
        name,
        key: String::new(),
        destination: registry::resolve(env, &entry.destination),
        root: entry.root,
        executable: mode & 0o111 != 0,
        hash: hash_bytes(content.as_bytes()),
        content,
        service: entry.service.clone().map(|(name, scope, enable)| ServiceRef { name, scope, enable }),
        out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activate::{Ask, Options, activate};
    use crate::runner::fake::FakeRunner;
    use crate::testing::{fixture, write};

    struct Nobody;

    impl Ask for Nobody {
        fn ask(&self, _: &str) -> Option<String> {
            None
        }
    }

    #[test]
    fn destinations_are_portable() {
        let env = Env::new(Path::new("/home/a"), Path::new("/"), Path::new("/s"));
        assert_eq!(portable(&env, Path::new("/home/a/.config/foot/foot.ini")), "~/.config/foot/foot.ini");
        assert_eq!(portable(&env, Path::new("/etc/greetd/config.toml")), "/etc/greetd/config.toml");
        let scratch = Env::new(Path::new("/h"), Path::new("/tmp/root"), Path::new("/s"));
        assert_eq!(portable(&scratch, Path::new("/tmp/root/etc/sv/x/run")), "/etc/sv/x/run");
    }

    #[test]
    fn another_machine_activates_from_the_index_without_nix() {
        let fixture = fixture(&["foot", "usersvc"]);
        write(&fixture.repo.static_dir().join("nvim/init.lua"), "-- lua\n");
        activate(&fixture.env, &fixture.runner, &fixture.repo, &Nobody, Options::default()).unwrap();

        // a second home, whose runner fails anything nix would be asked
        let other = Env::new(&fixture.dir.path().join("other"), &fixture.dir.path().join("sys2"), &fixture.env.share_dir);
        let no_nix = FakeRunner::fallible(|program, _| match program {
            "nix-instantiate" => panic!("nix was called"),
            _ => Ok(String::new()),
        });
        let options = Options { no_build: true, ..Options::default() };
        activate(&other, &no_nix, &fixture.repo, &Nobody, options).unwrap();

        assert_eq!(fs::read_link(other.home.join(".config/foot/foot.ini")).unwrap(), fixture.repo.out_dir().join("foot/foot.ini"));
        assert_eq!(fs::read_link(other.home.join(".config/nvim/init.lua")).unwrap(), fixture.repo.static_dir().join("nvim/init.lua"));
        assert_eq!(fs::read_link(other.home.join(".config/service/usersvc")).unwrap(), other.home.join(".config/sv/usersvc"));
    }

    #[test]
    fn no_index_is_an_error() {
        let fixture = fixture(&[]);
        assert!(matches!(load(&fixture.env, &fixture.repo), Err(IndexError::Missing(_))));
    }
}
