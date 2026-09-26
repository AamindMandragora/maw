use super::releases::{FORGES, rewrite};
use super::{Deps, Emitted, Resolved, SourceEmitter, SourcePkg};

// void's xbps-src template format
pub struct XbpsSrc;

// a double-quoted shell value, safe for any text
fn quote(text: &str) -> String {
    let escaped: String = text.chars().flat_map(|char| if "\"$`\\".contains(char) { vec!['\\', char] } else { vec![char] }).collect();
    format!("\"{escaped}\"")
}

// the distfile url with the version as ${version}, so a version bump is one line; the host, and a forge's
// owner/repo, are left as they are, since a version like "3" can be part of a name
fn templated(distfile: &str, version: &str) -> String {
    if version.is_empty() {
        return distfile.to_string();
    }
    let fixed = fixed_len(distfile);
    format!("{}{}", &distfile[..fixed], distfile[fixed..].replace(version, "${version}"))
}

// how much of a url never holds the version: scheme and host, and on a forge the owner and repo after it
fn fixed_len(url: &str) -> usize {
    let Some(after_scheme) = url.find("://").map(|index| index + 3) else { return 0 };
    let host = url[after_scheme..].split('/').next().unwrap_or_default();
    let segments = if FORGES.contains(&host) { 3 } else { 1 };
    url[after_scheme..].match_indices('/').nth(segments - 1).map_or(url.len(), |(index, _)| after_scheme + index)
}

// each upstream dependency resolved, in order and without repeats, plus the names nothing matched
fn resolve_all(names: &[String], deps: &Deps, devel: bool) -> (Vec<String>, Vec<String>) {
    names.iter().fold((Vec::new(), Vec::new()), |(mut packages, mut unknown), name| {
        match deps.resolve(name, devel) {
            Resolved::Package(package) if !packages.contains(&package) => packages.push(package),
            Resolved::Unknown if !unknown.contains(name) => unknown.push(name.clone()),
            _ => {}
        }
        (packages, unknown)
    })
}

// void's short_desc: no trailing period
fn short_desc(description: &str) -> String {
    description.trim().trim_end_matches('.').to_string()
}

impl SourceEmitter for XbpsSrc {
    fn emit(&self, pkg: &SourcePkg, deps: &Deps, maintainer: &str, checksum: &str) -> Emitted {
        let (host, host_todos) = resolve_all(&pkg.host_deps, deps, false);
        let (make, make_todos) = resolve_all(&pkg.deps, deps, true);
        let make: Vec<String> = make.into_iter().filter(|package| !host.contains(package)).collect();

        // mixed deps split into libraries to build against and programs to run with
        let (make, run): (Vec<String>, Vec<String>) = make.into_iter().partition(|package| !pkg.mixed_deps || package.ends_with("-devel"));
        let todos: Vec<String> = host_todos.into_iter().chain(make_todos).collect();

        // optional lines, present only when there's something to say
        let go_import_path = pkg.go_import_path.as_ref().map(|path| format!("go_import_path={}\n", quote(path)));
        let go_package = (!pkg.go_packages.is_empty()).then(|| format!("go_package={}\n", quote(&pkg.go_packages.join(" "))));
        let host_line = (!host.is_empty()).then(|| format!("hostmakedepends={}\n", quote(&host.join(" "))));
        let make_line = (!make.is_empty()).then(|| format!("makedepends={}\n", quote(&make.join(" "))));
        let run_line = (!run.is_empty()).then(|| format!("depends={}\n", quote(&run.join(" "))));
        let todo_lines: String = todos.iter().map(|todo| format!("# TODO: {} had '{todo}'\n", pkg.upstream)).chain(pkg.notes.iter().map(|note| format!("# TODO: {note}\n"))).collect();
        let optional: String = [go_import_path, go_package, host_line, make_line, run_line].into_iter().flatten().collect();

        let text = format!(
            "# Template file for '{name}'\n# scaffolded by maw from {origin}\npkgname={name}\nversion={version}\nrevision=1\nbuild_style={build}\n{optional}{todo_lines}short_desc={desc}\nmaintainer={maintainer}\nlicense={license}\nhomepage={homepage}\ndistfiles=\"{distfile}\"\nchecksum={checksum}\n{extra}",
            name = pkg.name,
            origin = pkg.origin,
            version = pkg.version,
            build = pkg.build,
            desc = quote(&short_desc(&pkg.description)),
            maintainer = quote(maintainer),
            license = quote(&pkg.licenses.join(", ")),
            homepage = quote(&pkg.homepage),
            distfile = templated(&pkg.distfile, &pkg.version),
            extra = pkg.extra,
        );
        Emitted { text, todos }
    }
}

// an existing template moved to a new version: version, revision, distfiles, checksum (multi-line values replaced
// whole), and the origin line; nothing else is touched
pub fn bump(template: &str, pkg: &SourcePkg, checksum: &str) -> String {
    let fields = [
        ("version", pkg.version.clone()),
        ("revision", "1".to_string()),
        ("distfiles", format!("\"{}\"", templated(&pkg.distfile, &pkg.version))),
        ("checksum", checksum.to_string()),
    ];
    let origin = |line: &str| if line.starts_with("# scaffolded by maw from ") { format!("# scaffolded by maw from {}\n", pkg.origin) } else { format!("{line}\n") };
    rewrite(template, &fields).lines().map(origin).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_escape_shell_specials() {
        assert_eq!(quote(r#"a "b" $c `d` \e"#), r#""a \"b\" \$c \`d\` \\e""#);
    }

    #[test]
    fn versions_in_urls_become_the_variable() {
        assert_eq!(templated("https://x.org/a/archive/v1.2.tar.gz", "1.2"), "https://x.org/a/archive/v${version}.tar.gz");
    }

    #[test]
    fn versions_in_forge_owners_and_repos_are_kept() {
        assert_eq!(templated("https://github.com/x/py3lib/archive/3.tar.gz", "3"), "https://github.com/x/py3lib/archive/${version}.tar.gz");
        assert_eq!(templated("https://files3.org/3/a-3.tar.gz", "3"), "https://files3.org/${version}/a-${version}.tar.gz");
    }

    #[test]
    fn bumps_replace_multiline_distfiles_and_checksums() {
        let template = "# scaffolded by maw from nixpkgs 'a' at 1\npkgname=a\nversion=1\nrevision=2\ndistfiles=\"https://x.org/a-1.tar.gz\n https://x.org/extra.tar.gz\"\nchecksum=\"old\n older\"\nshort_desc=\"a\"\n";
        let pkg = SourcePkg {
            name: "a".into(),
            version: "2".into(),
            description: String::new(),
            homepage: String::new(),
            licenses: Vec::new(),
            distfile: "https://x.org/a-2.tar.gz".into(),
            build: String::new(),
            host_deps: Vec::new(),
            deps: Vec::new(),
            go_import_path: None,
            go_packages: Vec::new(),
            origin: "nixpkgs 'a' at 2".into(),
            upstream: "nix".into(),
            mixed_deps: false,
            notes: Vec::new(),
            extra: String::new(),
        };
        assert_eq!(bump(template, &pkg, "new"), "# scaffolded by maw from nixpkgs 'a' at 2\npkgname=a\nversion=2\nrevision=1\ndistfiles=\"https://x.org/a-${version}.tar.gz\"\nchecksum=new\nshort_desc=\"a\"\n");
    }
}
