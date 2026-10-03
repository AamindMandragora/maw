use super::{Backend, BackendError, Pkg, spec_base};
use crate::env::Env;
use crate::runner::{RunError, Runner, as_root};
use std::path::{Path, PathBuf};

const FLATHUB: &str = "https://dl.flathub.org/repo/flathub.flatpakrepo";

// flatpak apps from flathub, declared by app id; a commit after @ pins an exact build
pub struct Flatpak<'a> {
    runner: &'a dyn Runner,
    sysroot: PathBuf,
    home: PathBuf,
    // where the installation keeps its apps: the system one, or a scratch root's user one
    installation: PathBuf,
}

impl<'a> Flatpak<'a> {
    pub fn new(runner: &'a dyn Runner, env: &Env) -> Self {
        let installation = if env.sysroot == Path::new("/") { PathBuf::from("/var/lib/flatpak") } else { env.home.join(".local/share/flatpak") };
        Flatpak { runner, sysroot: env.sysroot.clone(), home: env.home.clone(), installation }
    }

    // the system installation on the real root; a scratch root gets a user installation under its home, so it never touches the machine
    fn scope(&self) -> &str {
        if self.sysroot == Path::new("/") { "--system" } else { "--user" }
    }

    fn query(&self, args: &[&str]) -> Result<String, RunError> {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).chain([self.scope().to_string()]).collect();
        self.runner.run("flatpak", &args)
    }

    // a change to the installation, on the terminal: through sudo for the system one. Installs, updates, and
    // uninstalls answer yes to everything; remote-add takes no such flags
    fn change(&self, args: &[String]) -> Result<(), BackendError> {
        let unattended = ["install", "update", "uninstall"].contains(&args[0].as_str());
        let flags = [self.scope().to_string()].into_iter().chain(unattended.then(|| ["-y".to_string(), "--noninteractive".into()]).into_iter().flatten());
        let args: Vec<String> = args.iter().cloned().chain(flags).collect();
        Ok(as_root(self.runner, &self.sysroot, "flatpak", &args)?)
    }

    // the full commit an installed app is at: the name its deploy's `active` link points at, which flatpak info reads
    // too, else flatpak info itself
    fn commit(&self, id: &str) -> Option<String> {
        let active = std::fs::read_link(self.installation.join("app").join(id).join("current/active")).ok();
        let on_disk = active.and_then(|commit| Some(commit.file_name()?.to_string_lossy().into_owned())).filter(|commit| commit.len() == 64);
        on_disk.or_else(|| Some(self.query(&["info", "--show-commit", id]).ok()?.trim().to_string()).filter(|commit| !commit.is_empty()))
    }

    // an app from flathub's appstream api: None when flathub says it has no such app, the bare id when it can't be asked
    fn appstream(&self, id: &str) -> Option<Pkg> {
        let bare = Pkg { name: id.into(), source: id.into(), ..Pkg::default() };
        let body = match self.runner.run("curl", &["-sSf".into(), format!("https://flathub.org/api/v2/appstream/{id}")]) {
            Ok(body) => body,
            Err(RunError::Failed { stderr, .. }) if stderr.contains("404") => return None,
            Err(_) => return Some(bare),
        };
        let app: serde_json::Value = serde_json::from_str(&body).ok()?;
        let field = |value: &serde_json::Value| value.as_str().unwrap_or_default().to_string();
        let found = app["id"].as_str() == Some(id);
        found.then(|| Pkg { version: field(&app["releases"][0]["version"]), description: field(&app["summary"]), homepage: field(&app["urls"]["homepage"]), ..bare })
    }

    // updates apps to their latest builds, or all of them when none are named
    pub fn update(&self, ids: &[String]) -> Result<(), BackendError> {
        self.change(&["update".to_string()].into_iter().chain(ids.iter().cloned()).collect::<Vec<_>>())
    }
}

// "com.tomjwatson.Emote" -> "emote", the name its wrapper runs it by
pub fn short_name(id: &str) -> String {
    spec_base(id).rsplit('.').next().unwrap_or(id).to_lowercase()
}

// whether a bare request is a flatpak app id: reverse-dns, like com.slack.Slack
pub fn is_app_id(request: &str) -> bool {
    let parts: Vec<&str> = request.split('.').collect();
    parts.len() >= 3 && parts.iter().all(|part| !part.is_empty() && part.chars().all(|char| char.is_ascii_alphanumeric() || char == '_' || char == '-'))
}

// tab-separated columns of flatpak list or search
fn parse_rows(output: &str, manual: bool) -> Vec<Pkg> {
    let rows = output.lines().filter(|line| line.contains('\t'));
    rows.map(|line| {
        let columns: Vec<&str> = line.split('\t').map(str::trim).collect();
        let column = |index: usize| columns.get(index).copied().unwrap_or_default().to_string();
        Pkg { name: column(0), source: column(0), version: column(1), description: column(2), manual, ..Pkg::default() }
    })
    .collect()
}

impl Backend for Flatpak<'_> {
    fn name(&self) -> &str {
        "flatpak"
    }

    // installed apps with their commits; nothing when flatpak isn't installed
    fn list(&self) -> Result<Vec<Pkg>, BackendError> {
        let output = match self.query(&["list", "--app", "--columns=application,version,name"]) {
            Err(RunError::Spawn { .. }) => return Ok(Vec::new()),
            output => output?,
        };
        let pkgs = parse_rows(&output, true).into_iter().map(|pkg| Pkg { build: self.commit(&pkg.source).unwrap_or_default(), ..pkg });
        Ok(pkgs.collect())
    }

    // flathub's apps matching a term, from its appstream data; nothing when flatpak can't search
    fn search(&self, term: &str) -> Result<Vec<Pkg>, BackendError> {
        let output = self.runner.run("flatpak", &["search".into(), "--columns=application,version,description".into(), term.into()]);
        Ok(output.map(|output| parse_rows(&output, false)).unwrap_or_default())
    }

    // an app on flathub, at its current commit; without the flathub remote (install adds it), from flathub's api instead
    fn info(&self, id: &str) -> Result<Option<Pkg>, BackendError> {
        let output = match self.runner.run("flatpak", &["remote-info".into(), "flathub".into(), spec_base(id).into()]) {
            Ok(output) => output,
            Err(RunError::Failed { stderr, .. }) if stderr.contains("\"flathub\" not found") => return Ok(self.appstream(spec_base(id))),
            Err(RunError::Failed { .. }) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let commit = output.lines().find_map(|line| line.trim().strip_prefix("Commit: ")).unwrap_or_default();
        Ok(Some(Pkg { name: spec_base(id).into(), source: spec_base(id).into(), version: commit.chars().take(12).collect(), build: commit.into(), ..Pkg::default() }))
    }

    // adds flathub if it's missing, then installs; a pinned app is moved to its commit afterwards
    fn install(&self, specs: &[String]) -> Result<(), BackendError> {
        self.change(&["remote-add".into(), "--if-not-exists".into(), "flathub".into(), FLATHUB.into()])?;
        let ids: Vec<String> = specs.iter().map(|spec| spec_base(spec).to_string()).collect();
        self.change(&["install".to_string(), "--or-update".into(), "flathub".into()].into_iter().chain(ids).collect::<Vec<_>>())?;

        // "id@commit" -> update --commit, one app at a time since a commit belongs to one app
        let pinned = specs.iter().filter_map(|spec| spec.split_once('@'));
        pinned.into_iter().try_for_each(|(id, commit)| self.change(&["update".into(), format!("--commit={commit}"), id.into()]))
    }

    // uninstalls the apps, then runtimes nothing uses any more
    fn remove(&self, specs: &[String]) -> Result<(), BackendError> {
        let ids = specs.iter().map(|spec| spec_base(spec).to_string());
        self.change(&["uninstall".to_string()].into_iter().chain(ids).collect::<Vec<_>>())?;
        self.change(&["uninstall".into(), "--unused".into()])
    }

    fn tool(&self) -> Option<(&'static str, &'static str)> {
        Some(("flatpak", "flatpak"))
    }

    // the app at its commit, or bare when the commit couldn't be read
    fn pin(&self, pkg: &Pkg) -> String {
        if pkg.build.is_empty() { pkg.source.clone() } else { format!("{}@{}", pkg.source, pkg.build) }
    }

    // where maw writes each app's short-name wrapper
    fn bin_dir(&self) -> Option<PathBuf> {
        Some(self.home.join(".local/bin"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::FakeRunner;

    const LIST: &str = "com.slack.Slack\t4.52.155\tSlack\ncom.tomjwatson.Emote\t4.0.2\tEmote\n";

    fn fake(program: &str, args: &[String]) -> String {
        match (program, args[0].as_str()) {
            ("flatpak", "list") => LIST.into(),
            ("flatpak", "info") => format!("{}c0ffee\n", args[2].len()),
            ("flatpak", "remote-info") => "        ID: com.tomjwatson.Emote\n    Commit: aa30cd8385982370cb71\n".into(),
            _ => String::new(),
        }
    }

    fn env(sysroot: &str) -> Env {
        Env::new(Path::new("/h"), Path::new(sysroot), Path::new("/s"))
    }

    #[test]
    fn commits_come_from_the_deploy_link() {
        let dir = tempfile::tempdir().unwrap();
        let commit = "9ca1af9262bd6f208c8e247bce01d1300eabd2342d19a2b8c92f9cd4d541fc77";
        let current = dir.path().join("home/.local/share/flatpak/app/com.slack.Slack/current");
        std::fs::create_dir_all(&current).unwrap();
        std::os::unix::fs::symlink(commit, current.join("active")).unwrap();
        let runner = FakeRunner::new(|_, _| String::new());
        let flatpak = Flatpak::new(&runner, &env(&dir.path().join("sys").display().to_string()));
        let flatpak = Flatpak { installation: dir.path().join("home/.local/share/flatpak"), ..flatpak };
        assert_eq!(flatpak.commit("com.slack.Slack").as_deref(), Some(commit));
        assert!(runner.calls.borrow().is_empty());
    }

    #[test]
    fn list_reads_apps_and_their_commits() {
        let runner = FakeRunner::new(fake);
        // no deploy links to read, so the commits come from flatpak info
        let apps = Flatpak { installation: PathBuf::from("/nonexistent"), ..Flatpak::new(&runner, &env("/")) }.list().unwrap();
        assert_eq!(apps.iter().map(|app| (app.source.as_str(), app.version.as_str(), app.build.as_str())).collect::<Vec<_>>(), [
            ("com.slack.Slack", "4.52.155", "15c0ffee"),
            ("com.tomjwatson.Emote", "4.0.2", "20c0ffee")
        ]);
        assert_eq!(runner.calls.borrow()[0], "flatpak list --app --columns=application,version,name --system");
        assert_eq!(Flatpak::new(&runner, &env("/")).pin(&apps[1]), "com.tomjwatson.Emote@20c0ffee");
    }

    #[test]
    fn info_reads_the_remote_commit() {
        let runner = FakeRunner::new(fake);
        let emote = Flatpak::new(&runner, &env("/")).info("com.tomjwatson.Emote").unwrap().unwrap();
        assert_eq!((emote.version.as_str(), emote.build.as_str()), ("aa30cd838598", "aa30cd8385982370cb71"));
    }

    #[test]
    fn without_the_remote_info_asks_flathubs_api() {
        let runner = FakeRunner::fallible(|program, args| match (program, args.last().map(String::as_str)) {
            ("flatpak", _) => Err(RunError::Failed { program: "flatpak".into(), stderr: "error: Remote \"flathub\" not found".into() }),
            ("curl", Some(url)) if url.ends_with("com.slack.Slack") => Ok(r#"{"id":"com.slack.Slack","summary":"Business communication","releases":[{"version":"4.52.162"}]}"#.into()),
            ("curl", Some(url)) if url.ends_with("com.nope.Nope") => Err(RunError::Failed { program: "curl".into(), stderr: "curl: (22) The requested URL returned error: 404".into() }),
            _ => Err(RunError::Failed { program: "curl".into(), stderr: "curl: (6) Could not resolve host".into() }),
        });
        let flatpak = Flatpak::new(&runner, &env("/"));
        let slack = flatpak.info("com.slack.Slack").unwrap().unwrap();
        assert_eq!((slack.version.as_str(), slack.description.as_str()), ("4.52.162", "Business communication"));
        assert!(flatpak.info("com.nope.Nope").unwrap().is_none());
        assert_eq!(flatpak.info("com.offline.App").unwrap().unwrap().source, "com.offline.App");
    }

    #[test]
    fn unreadable_commits_pin_nothing() {
        let runner = FakeRunner::new(fake);
        let app = Pkg { source: "com.slack.Slack".into(), ..Pkg::default() };
        assert_eq!(Flatpak::new(&runner, &env("/")).pin(&app), "com.slack.Slack");
    }

    #[test]
    fn system_changes_go_through_sudo_and_pins_move_to_their_commit() {
        let runner = FakeRunner::new(fake);
        Flatpak::new(&runner, &env("/")).install(&["com.slack.Slack".into(), "com.tomjwatson.Emote@abc".into()]).unwrap();
        assert_eq!(runner.calls.borrow()[..], [
            format!("sudo flatpak remote-add --if-not-exists flathub {FLATHUB} --system"),
            "sudo flatpak install --or-update flathub com.slack.Slack com.tomjwatson.Emote --system -y --noninteractive".to_string(),
            "sudo flatpak update --commit=abc com.tomjwatson.Emote --system -y --noninteractive".to_string(),
        ]);
    }

    #[test]
    fn a_scratch_root_uses_a_user_installation_without_sudo() {
        let runner = FakeRunner::new(fake);
        Flatpak::new(&runner, &env("/tmp/root")).remove(&["com.slack.Slack".into()]).unwrap();
        assert_eq!(runner.calls.borrow()[..], [
            "flatpak uninstall com.slack.Slack --user -y --noninteractive",
            "flatpak uninstall --unused --user -y --noninteractive"
        ]);
    }

    #[test]
    fn app_ids_and_short_names() {
        assert!(is_app_id("com.slack.Slack") && is_app_id("org.vinegarhq.Sober"));
        assert!(!is_app_id("foot") && !is_app_id("python3.12") && !is_app_id("github.com/x/y"));
        assert_eq!(short_name("com.tomjwatson.Emote@abc"), "emote");
    }
}
