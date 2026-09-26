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
}

impl<'a> Flatpak<'a> {
    pub fn new(runner: &'a dyn Runner, env: &Env) -> Self {
        Flatpak { runner, sysroot: env.sysroot.clone(), home: env.home.clone() }
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

    // the full commit an installed app is at
    fn commit(&self, id: &str) -> Option<String> {
        Some(self.query(&["info", "--show-commit", id]).ok()?.trim().to_string()).filter(|commit| !commit.is_empty())
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

    // an app on flathub, at its current commit
    fn info(&self, id: &str) -> Result<Option<Pkg>, BackendError> {
        let output = match self.runner.run("flatpak", &["remote-info".into(), "flathub".into(), spec_base(id).into()]) {
            Ok(output) => output,
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

    fn pin(&self, pkg: &Pkg) -> String {
        format!("{}@{}", pkg.source, pkg.build)
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
    fn list_reads_apps_and_their_commits() {
        let runner = FakeRunner::new(fake);
        let apps = Flatpak::new(&runner, &env("/")).list().unwrap();
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
