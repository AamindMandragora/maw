use super::{Deps, Emitted, Resolved, SourceEmitter, SourcePkg};

// void's xbps-src template format
pub struct XbpsSrc;

// a double-quoted shell value, safe for any text
fn quote(text: &str) -> String {
    let escaped: String = text.chars().flat_map(|char| if "\"$`\\".contains(char) { vec!['\\', char] } else { vec![char] }).collect();
    format!("\"{escaped}\"")
}

// the distfile url with the version as ${version}, so a version bump is one line
fn templated(distfile: &str, version: &str) -> String {
    if version.is_empty() { distfile.to_string() } else { distfile.replace(version, "${version}") }
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
        let todos: Vec<String> = host_todos.into_iter().chain(make_todos).collect();

        // optional lines, present only when there's something to say
        let go_import_path = pkg.go_import_path.as_ref().map(|path| format!("go_import_path={}\n", quote(path)));
        let go_package = (!pkg.go_packages.is_empty()).then(|| format!("go_package={}\n", quote(&pkg.go_packages.join(" "))));
        let host_line = (!host.is_empty()).then(|| format!("hostmakedepends={}\n", quote(&host.join(" "))));
        let make_line = (!make.is_empty()).then(|| format!("makedepends={}\n", quote(&make.join(" "))));
        let todo_lines: String = todos.iter().map(|todo| format!("# TODO: nix had '{todo}'\n")).collect();
        let optional: String = [go_import_path, go_package, host_line, make_line].into_iter().flatten().collect();

        let text = format!(
            "# Template file for '{name}'\n# scaffolded by maw from {origin}\npkgname={name}\nversion={version}\nrevision=1\nbuild_style={build}\n{optional}{todo_lines}short_desc={desc}\nmaintainer={maintainer}\nlicense={license}\nhomepage={homepage}\ndistfiles=\"{distfile}\"\nchecksum={checksum}\n",
            name = pkg.name,
            origin = pkg.origin,
            version = pkg.version,
            build = pkg.build,
            desc = quote(&short_desc(&pkg.description)),
            maintainer = quote(maintainer),
            license = quote(&pkg.licenses.join(", ")),
            homepage = quote(&pkg.homepage),
            distfile = templated(&pkg.distfile, &pkg.version),
        );
        Emitted { text, todos }
    }
}

// an existing template moved to a new version: version, revision, distfiles, checksum, and the origin line; nothing else is touched
pub fn bump(template: &str, pkg: &SourcePkg, checksum: &str) -> String {
    template
        .lines()
        .map(|line| match line.split_once('=').map(|(key, _)| key) {
            Some("version") => format!("version={}", pkg.version),
            Some("revision") => "revision=1".to_string(),
            Some("distfiles") => format!("distfiles=\"{}\"", templated(&pkg.distfile, &pkg.version)),
            Some("checksum") => format!("checksum={checksum}"),
            _ if line.starts_with("# scaffolded by maw from ") => format!("# scaffolded by maw from {}", pkg.origin),
            _ => line.to_string(),
        })
        .map(|line| line + "\n")
        .collect()
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
}
