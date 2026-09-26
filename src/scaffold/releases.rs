use super::{ScaffoldError, checksum};
use crate::backend::newer;
use crate::env::Env;
use crate::runner::Runner;

// templates that follow their upstream's releases: ones whose download is a tagged release on a git forge, with
// ${version} in its url. New versions are the repo's tags, read with git ls-remote (no api, no rate limit)

const FORGES: [&str; 4] = ["github.com", "gitlab.com", "codeberg.org", "git.sr.ht"];

// a line that keeps a template where it is
const HOLD: &str = "# maw: hold";

// where a template's releases come from: the repo, and the tag around ${version}, like v${version}
#[derive(Debug, PartialEq)]
pub struct Tracked {
    pub repo: String,
    pub prefix: String,
    pub suffix: String,
    // the download with only ${version} left to fill in
    pub url: String,
}

// a template field's value, quotes removed
pub fn field(template: &str, key: &str) -> Option<String> {
    let value = template.lines().find_map(|line| line.trim().strip_prefix(key)?.strip_prefix('='))?;
    Some(value.trim().trim_matches(|char| char == '"' || char == '\'').to_string())
}

// the release a template follows, if it downloads one from a forge and isn't held
pub fn tracked(template: &str) -> Option<Tracked> {
    if template.lines().any(|line| line.trim() == HOLD) {
        return None;
    }

    // the first download, with the fields it's written in terms of filled in
    let distfile = field(template, "distfiles")?.split_whitespace().next()?.split('>').next()?.to_string();
    let filled = ["homepage", "pkgname"].iter().fold(distfile, |url, key| match field(template, key) {
        Some(value) => url.replace(&format!("${{{key}}}"), &value),
        None => url,
    });

    // https://<forge>/<owner>/<repo>/..., and the first path part after the repo holding the version
    let path: Vec<&str> = filled.strip_prefix("https://")?.split('/').collect();
    if path.len() < 4 || !FORGES.contains(&path[0]) {
        return None;
    }
    let repo = format!("https://{}/{}/{}", path[0], path[1], path[2].trim_end_matches(".git"));
    let part = path[3..].iter().find(|part| part.contains("${version}"))?;
    let tag = [".tar.gz", ".tar.xz", ".tar.bz2", ".tgz", ".zip"].iter().fold(part.to_string(), |tag, ext| tag.trim_end_matches(ext).to_string());
    let (prefix, suffix) = tag.split_once("${version}")?;
    Some(Tracked { repo, prefix: prefix.into(), suffix: suffix.into(), url: filled })
}

// the highest version among the repo's tags that fit the template's tag; pre-releases (anything but digits and dots) don't count
pub fn latest(runner: &dyn Runner, tracked: &Tracked) -> Result<Option<String>, ScaffoldError> {
    let output = runner.run("git", &["ls-remote".into(), "--tags".into(), "--refs".into(), tracked.repo.clone()])?;
    let versions = output.lines().filter_map(|line| {
        let tag = line.split_whitespace().nth(1)?.strip_prefix("refs/tags/")?;
        let version = tag.strip_prefix(tracked.prefix.as_str())?.strip_suffix(tracked.suffix.as_str())?;
        let release = version.starts_with(|char: char| char.is_ascii_digit()) && version.chars().all(|char| char.is_ascii_digit() || char == '.');
        release.then(|| version.to_string())
    });
    Ok(versions.reduce(|best, version| if newer(&version, &best) { version } else { best }))
}

// moves a template to its upstream's newest release: version, revision, and checksum; Some((old, new)) if it moved
pub fn update(env: &Env, runner: &dyn Runner, file: &std::path::Path) -> Result<Option<(String, String)>, ScaffoldError> {
    let text = std::fs::read_to_string(file).map_err(|source| ScaffoldError::Io { path: file.into(), source })?;
    let Some(tracked) = tracked(&text) else { return Ok(None) };
    let current = field(&text, "version").unwrap_or_default();
    let Some(latest) = latest(runner, &tracked)?.filter(|latest| newer(latest, &current)) else { return Ok(None) };

    let sum = checksum(env, runner, &tracked.url.replace("${version}", &latest))?;
    let bumped: String = text
        .lines()
        .map(|line| match line.split_once('=').map(|(key, _)| key) {
            Some("version") => format!("version={latest}"),
            Some("revision") => "revision=1".to_string(),
            Some("checksum") => format!("checksum={sum}"),
            _ => line.to_string(),
        })
        .map(|line| line + "\n")
        .collect();
    std::fs::write(file, bumped).map_err(|source| ScaffoldError::Io { path: file.into(), source })?;
    Ok(Some((current, latest)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::FakeRunner;
    use crate::testing::{fixture, write};

    const MAW: &str = "pkgname=maw\nversion=0.1.1\nrevision=2\nhomepage=\"https://github.com/AamindMandragora/maw\"\ndistfiles=\"${homepage}/archive/refs/tags/v${version}.tar.gz\"\nchecksum=old\n";

    #[test]
    fn forge_downloads_are_tracked_by_their_tag() {
        let maw = tracked(MAW).unwrap();
        assert_eq!((maw.repo.as_str(), maw.prefix.as_str(), maw.suffix.as_str()), ("https://github.com/AamindMandragora/maw", "v", ""));
        assert_eq!(maw.url, "https://github.com/AamindMandragora/maw/archive/refs/tags/v${version}.tar.gz");

        let yay = tracked("distfiles=\"https://github.com/Jguer/yay/releases/download/v${version}/yay_${version}_x86_64.tar.gz\"\n").unwrap();
        assert_eq!((yay.prefix.as_str(), yay.suffix.as_str()), ("v", ""));
        assert!(tracked("distfiles=\"https://download.gnome.org/sources/x/${version}.tar.xz\"\n").is_none());
        assert!(tracked(&format!("{MAW}{HOLD}\n")).is_none());
    }

    #[test]
    fn the_newest_matching_tag_wins() {
        let runner = FakeRunner::new(|_, _| "a\trefs/tags/v0.9.0\nb\trefs/tags/v0.10.0\nc\trefs/tags/nightly\nd\trefs/tags/v0.11.0-rc1\n".into());
        let maw = tracked(MAW).unwrap();
        assert_eq!(latest(&runner, &maw).unwrap().as_deref(), Some("0.10.0"));
        let strict = Tracked { suffix: String::new(), ..maw };
        assert!(latest(&FakeRunner::new(|_, _| "a\trefs/tags/nightly\n".into()), &strict).unwrap().is_none());
    }

    #[test]
    fn update_moves_version_revision_and_checksum_only() {
        let fixture = fixture(&[]);
        let file = fixture.repo.srcpkgs_dir().join("maw/template");
        write(&file, MAW);
        let runner = FakeRunner::new(|program, _| match program {
            "git" => "a\trefs/tags/v0.1.1\nb\trefs/tags/v0.2.0\n".into(),
            "sha256sum" => "abc123  file\n".into(),
            _ => String::new(),
        });
        assert_eq!(update(&fixture.env, &runner, &file).unwrap(), Some(("0.1.1".into(), "0.2.0".into())));
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("version=0.2.0\nrevision=1\n") && text.contains("checksum=abc123\n") && text.contains("distfiles=\"${homepage}"));
        assert!(runner.calls.borrow().iter().any(|call| call.contains("https://github.com/AamindMandragora/maw/archive/refs/tags/v0.2.0.tar.gz")));
        assert_eq!(update(&fixture.env, &FakeRunner::new(|_, _| "a\trefs/tags/v0.2.0\n".into()), &file).unwrap(), None);
    }
}
