use super::{ScaffoldError, SourcePkg, go_import_path};
use crate::runner::Runner;
use serde_json::Value;
use std::collections::BTreeMap;

// a PKGBUILD read without running it: top-level variables and function bodies. maw never executes AUR code,
// so anything past plain assignments and simple ${var/a/b} expansions is left for the user as a TODO
pub struct Pkgbuild {
    vars: BTreeMap<String, Vec<String>>,
    text: String,
}

// the architecture maw builds for, as PKGBUILDs spell it
const CARCH: &str = "x86_64";

impl Pkgbuild {
    pub fn parse(text: &str) -> Self {
        let mut vars = BTreeMap::new();
        let mut lines = text.lines();

        // assignments at the top level; function bodies (closed by a "}" in the first column) are skipped
        while let Some(line) = lines.next() {
            if function_name(line).is_some() {
                lines.by_ref().find(|line| line.starts_with('}'));
                continue;
            }
            let Some((name, value)) = assignment(line) else { continue };
            let mut value = value.to_string();
            if value.starts_with('(') {
                while !closes(&value) {
                    let Some(next) = lines.next() else { break };
                    value = format!("{value}\n{next}");
                }
                value = value.trim_start_matches('(').trim_end().trim_end_matches(')').to_string();
            }
            vars.insert(name.to_string(), words(&value));
        }
        Pkgbuild { vars, text: text.to_string() }
    }

    // a variable's first value, expanded
    pub fn var(&self, name: &str) -> Option<String> {
        self.vars.get(name)?.first().map(|value| self.expand(value, 0))
    }

    // every value of an array, expanded
    pub fn array(&self, name: &str) -> Vec<String> {
        self.vars.get(name).into_iter().flatten().map(|value| self.expand(value, 0)).collect()
    }

    // $var, ${var}, and ${var/a/b} ${var//a/b} ${var%a} ${var#a} with literal patterns; anything else stays as written
    fn expand(&self, text: &str, depth: usize) -> String {
        if depth > 8 || !text.contains('$') {
            return text.to_string();
        }
        let mut out = String::new();
        let mut rest = text;
        while let Some(start) = rest.find('$') {
            out.push_str(&rest[..start]);
            let after = &rest[start + 1..];
            let (expression, consumed) = match after.strip_prefix('{') {
                Some(braced) => match braced.find('}') {
                    Some(end) => (&braced[..end], end + 2),
                    None => ("", 0),
                },
                None => {
                    let end = after.find(|char: char| !(char.is_ascii_alphanumeric() || char == '_')).unwrap_or(after.len());
                    (&after[..end], end)
                }
            };
            match self.substitute(expression, depth) {
                Some(value) if consumed > 0 => out.push_str(&value),
                _ => out.push_str(&rest[start..start + 1 + consumed]),
            }
            rest = &rest[start + 1 + consumed..];
        }
        out.push_str(rest);
        out
    }

    // one ${...} expression's value, if it's a known variable with at most a literal substitution
    fn substitute(&self, expression: &str, depth: usize) -> Option<String> {
        let name_end = expression.find(|char: char| !(char.is_ascii_alphanumeric() || char == '_')).unwrap_or(expression.len());
        let (name, operation) = expression.split_at(name_end);
        let value = match name {
            "CARCH" => CARCH.to_string(),
            "srcdir" | "pkgdir" | "startdir" => return None,
            _ => self.expand(self.vars.get(name)?.first()?, depth + 1),
        };
        let literal = |pattern: &str| (!pattern.contains(['*', '?', '['])).then(|| pattern.to_string());
        match operation.chars().next() {
            None => Some(value),
            Some('/') => {
                let all = operation.starts_with("//");
                let (pattern, replacement) = operation.trim_start_matches('/').split_once('/').unwrap_or((operation.trim_start_matches('/'), ""));
                let pattern = literal(pattern)?;
                Some(if all { value.replace(&pattern, replacement) } else { value.replacen(&pattern, replacement, 1) })
            }
            Some('%') => Some(value.strip_suffix(&literal(operation.trim_start_matches('%'))?).map(String::from).unwrap_or(value)),
            Some('#') => Some(value.strip_prefix(&literal(operation.trim_start_matches('#'))?).map(String::from).unwrap_or(value)),
            _ => None,
        }
    }

    // a function's body, as written
    pub fn function(&self, name: &str) -> Option<String> {
        let mut lines = self.text.lines().skip_while(|line| function_name(line) != Some(name));
        lines.next()?;
        Some(lines.take_while(|line| !line.starts_with('}')).collect::<Vec<_>>().join("\n"))
    }
}

// "build() {" or "build () {" -> build
fn function_name(line: &str) -> Option<&str> {
    let (name, rest) = line.split_once('(')?;
    let name = name.trim_end();
    let valid = !name.is_empty() && name.chars().all(|char| char.is_ascii_alphanumeric() || char == '_') && !line.starts_with(char::is_whitespace);
    (valid && rest.trim_start().starts_with(')')).then_some(name)
}

// "name=value" at the start of a line
fn assignment(line: &str) -> Option<(&str, &str)> {
    let (name, value) = line.split_once('=')?;
    let valid = !name.is_empty() && name.chars().all(|char| char.is_ascii_alphanumeric() || char == '_');
    valid.then_some((name, value))
}

// whether an array's text has reached its closing paren, outside quotes and comments
fn closes(text: &str) -> bool {
    let unquoted = strip_quoted(text);
    unquoted.lines().map(|line| line.split('#').next().unwrap_or("")).any(|line| line.contains(')'))
}

// the text with quoted parts blanked, for finding structure
fn strip_quoted(text: &str) -> String {
    let mut quote = None;
    text.chars()
        .map(|char| match (quote, char) {
            (None, '\'' | '"') => {
                quote = Some(char);
                ' '
            }
            (Some(open), _) if char == open => {
                quote = None;
                ' '
            }
            (Some(_), '\n') => '\n',
            (Some(_), _) => ' ',
            _ => char,
        })
        .collect()
}

// shell words: quotes removed, comments dropped
fn words(text: &str) -> Vec<String> {
    let (mut words, mut current, mut quote, mut started) = (Vec::new(), String::new(), None, false);
    let mut chars = text.chars().peekable();
    while let Some(char) = chars.next() {
        match (quote, char) {
            (None, '#') if !started => {
                chars.by_ref().find(|char| *char == '\n');
            }
            (None, '\'' | '"') => (quote, started) = (Some(char), true),
            (Some(open), _) if char == open => quote = None,
            (None, char) if char.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut current));
                }
                started = false;
            }
            _ => (current, started) = (current + &char.to_string(), true),
        }
    }
    if started {
        words.push(current);
    }
    words
}

// a dependency without its version constraint or description: "pacman>6.1" -> pacman
fn dep_name(dep: &str) -> String {
    dep.split(['<', '>', '=', ':']).next().unwrap_or(dep).trim().to_string()
}

// a source entry that's a git checkout rather than a release
fn is_vcs(source: &str) -> bool {
    let url = source.rsplit("::").next().unwrap_or(source);
    ["git+", "git://", "svn+", "hg+", "bzr+"].iter().any(|prefix| url.starts_with(prefix)) || url.contains("#tag=") || url.contains("#commit=")
}

// void's build_style for how a PKGBUILD builds, or "" when it only repackages files
fn build_style(makedepends: &[String], build: &str, package: &str) -> &'static str {
    let needs = |names: &[&str]| makedepends.iter().any(|dep| names.contains(&dep.as_str()));
    let script = format!("{build}\n{package}");
    match () {
        _ if needs(&["meson"]) || script.contains("meson ") => "meson",
        _ if needs(&["cmake"]) || script.contains("cmake ") => "cmake",
        _ if needs(&["cargo", "rust"]) || script.contains("cargo build") => "cargo",
        _ if needs(&["go"]) || script.contains("go build") => "go",
        _ if needs(&["python-build", "python-installer"]) || script.contains("python -m build") => "python3-pep517",
        _ if build.contains("./configure") => "gnu-configure",
        _ if build.contains("make") => "gnu-makefile",
        _ => "",
    }
}

// a PKGBUILD function kept as comments at the end of the template, its common indent turned into one tab
fn reference(name: &str, body: &str) -> String {
    let indent = body.lines().filter(|line| !line.trim().is_empty()).map(|line| line.len() - line.trim_start().len()).min().unwrap_or(0);
    let lines: String = body.lines().map(|line| if line.trim().is_empty() { "#\n".to_string() } else { format!("#\t{}\n", &line[indent..]) }).collect();
    format!("\n# the PKGBUILD's {name}(), for anything the build style doesn't cover:\n{lines}")
}

// the aur's metadata for a package, from its rpc api
pub fn info(runner: &dyn Runner, name: &str) -> Result<Value, ScaffoldError> {
    let url = format!("https://aur.archlinux.org/rpc/v5/info?arg[]={name}");
    let body = runner.run("curl", &["-fsSL".into(), url])?;
    let response: Value = serde_json::from_str(&body).map_err(|source| ScaffoldError::Meta { attr: name.into(), source })?;
    response["results"].get(0).cloned().ok_or_else(|| ScaffoldError::NotInAur(name.into()))
}

// the PKGBUILD of a package base, from the aur's git web view
pub fn pkgbuild(runner: &dyn Runner, base: &str) -> Result<String, ScaffoldError> {
    Ok(runner.run("curl", &["-fsSL".into(), format!("https://aur.archlinux.org/cgit/aur.git/plain/PKGBUILD?h={base}")])?)
}

// a source package from an aur package's metadata and PKGBUILD
pub fn source_pkg(info: &Value, text: &str, name: &str) -> Result<SourcePkg, ScaffoldError> {
    let pkgbuild = Pkgbuild::parse(text);
    let field = |key: &str| info[key].as_str().unwrap_or_default().to_string();
    let list = |key: &str| info[key].as_array().into_iter().flatten().filter_map(|dep| Some(dep_name(dep.as_str()?))).collect::<Vec<_>>();
    let aur_name = field("Name");

    // the first release download; a checkout is refused, since xbps-src builds from tarballs
    let sources: Vec<String> = pkgbuild.array("source").into_iter().chain(pkgbuild.array(&format!("source_{CARCH}"))).collect();
    let is_signature = |source: &String| [".sig", ".asc", ".sign"].iter().any(|ext| source.ends_with(ext));
    let downloads: Vec<&String> = sources.iter().filter(|source| source.contains("://") && !is_signature(source)).collect();
    let distfile = match downloads.iter().find(|source| !is_vcs(source)) {
        Some(source) => source.rsplit("::").next().unwrap_or(source).to_string(),
        None if downloads.iter().any(|source| is_vcs(source)) => return Err(ScaffoldError::Vcs(aur_name)),
        None => return Err(ScaffoldError::NoSource(aur_name)),
    };

    // things a draft can't carry over on its own: local files the PKGBUILD ships, and expansions left unresolved
    let local = sources.iter().filter(|source| !source.contains("://")).map(|file| format!("the PKGBUILD also uses {file} from https://aur.archlinux.org/cgit/aur.git/tree/?h={}", field("PackageBase")));
    let unresolved = distfile.contains('$').then(|| format!("distfiles has an expansion maw couldn't resolve: {distfile}"));
    let arch_only = pkgbuild.array("source").iter().all(|source| !source.contains("://")).then(|| format!("the download is for {CARCH} only; add archs=\"{CARCH}\""));

    // the aur's version is "epoch:pkgver-pkgrel"; void wants pkgver alone
    let version = field("Version");
    let version = version.rsplit_once('-').map_or(version.as_str(), |(pkgver, _)| pkgver);
    let version = version.split_once(':').map_or(version, |(_, pkgver)| pkgver).to_string();

    let (build, package) = (pkgbuild.function("build").unwrap_or_default(), pkgbuild.function("package").or_else(|| pkgbuild.function(&format!("package_{aur_name}"))).unwrap_or_default());
    let makedepends = list("MakeDepends");
    let style = build_style(&makedepends, &build, &package);
    let unbuilt = style.is_empty().then(|| "no build style matched; port package() below into do_install()".to_string());
    let notes = local.chain(unresolved).chain(arch_only).chain(unbuilt).collect();
    let reference = [(style.is_empty() && !build.is_empty()).then(|| reference("build", &build)), (!package.is_empty()).then(|| reference("package", &package))];
    let homepage = field("URL");
    Ok(SourcePkg {
        name: name.into(),
        version: version.clone(),
        description: field("Description"),
        homepage: homepage.clone(),
        licenses: info["License"].as_array().into_iter().flatten().filter_map(|license| Some(license.as_str()?.to_string())).collect(),
        go_import_path: (style == "go").then(|| go_import_path(&homepage, &distfile)).flatten(),
        distfile,
        build: style.into(),
        host_deps: makedepends,
        deps: list("Depends"),
        go_packages: Vec::new(),
        origin: format!("aur '{aur_name}' at {}", field("Version")),
        upstream: "aur".into(),
        mixed_deps: true,
        notes,
        extra: reference.into_iter().flatten().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PKGBUILD: &str = r#"# Maintainer: someone
pkgname=tool-bin
_name=tool
pkgver=1.2.3
url="https://github.com/a/${_name}"
source=("$_name-$pkgver.tar.gz::https://github.com/a/tool/archive/v$pkgver.tar.gz" # the release
        'tool.sig')
source_x86_64=("https://x.org/${pkgname/-bin/}_${pkgver}_${CARCH}.tar.gz")
depends=(
  'glibc'
  "gtk3>=3.24"
)

build () {
  cd "$srcdir/$_name-$pkgver"
  make
}

package() {
  install -Dm755 tool "$pkgdir/usr/bin/tool"
}
"#;

    #[test]
    fn variables_and_arrays_parse_without_running_anything() {
        let pkgbuild = Pkgbuild::parse(PKGBUILD);
        assert_eq!(pkgbuild.var("url").as_deref(), Some("https://github.com/a/tool"));
        assert_eq!(pkgbuild.array("source"), ["tool-1.2.3.tar.gz::https://github.com/a/tool/archive/v1.2.3.tar.gz", "tool.sig"]);
        assert_eq!(pkgbuild.array("source_x86_64"), ["https://x.org/tool_1.2.3_x86_64.tar.gz"]);
        assert_eq!(pkgbuild.array("depends"), ["glibc", "gtk3>=3.24"]);
        assert_eq!(pkgbuild.function("build").unwrap(), "  cd \"$srcdir/$_name-$pkgver\"\n  make");
    }

    #[test]
    fn unknown_expansions_stay_as_written() {
        let pkgbuild = Pkgbuild::parse("a=${nope}\nb=${pkgver:0:3}\npkgver=1.0\nc=$srcdir/x\n");
        assert_eq!((pkgbuild.var("a").unwrap(), pkgbuild.var("b").unwrap(), pkgbuild.var("c").unwrap()), ("${nope}".into(), "${pkgver:0:3}".into(), "$srcdir/x".into()));
    }

    #[test]
    fn a_source_package_takes_the_release_download() {
        let info = json!({ "Name": "tool-bin", "PackageBase": "tool-bin", "Version": "1:1.2.3-2", "Description": "a tool", "URL": "https://x.org", "License": ["MIT"], "Depends": ["glibc", "gtk3>=3.24"], "MakeDepends": [] });
        let pkg = source_pkg(&info, PKGBUILD, "tool").unwrap();
        assert_eq!((pkg.version.as_str(), pkg.distfile.as_str(), pkg.build.as_str()), ("1.2.3", "https://github.com/a/tool/archive/v1.2.3.tar.gz", "gnu-makefile"));
        assert_eq!(pkg.deps, ["glibc", "gtk3"]);
        assert!(pkg.extra.contains("# the PKGBUILD's package()\n") || pkg.extra.contains("# the PKGBUILD's package(),"));
        assert!(pkg.extra.contains("#\tinstall -Dm755 tool"));
    }

    #[test]
    fn checkouts_are_refused() {
        let info = json!({ "Name": "tool-git", "PackageBase": "tool-git", "Version": "r1-1" });
        assert!(matches!(source_pkg(&info, "source=(\"git+https://github.com/a/tool\")\n", "tool"), Err(ScaffoldError::Vcs(_))));
    }

    #[test]
    fn build_styles_come_from_tools_and_scripts() {
        assert_eq!(build_style(&["meson".into()], "", ""), "meson");
        assert_eq!(build_style(&[], "cargo build --release", ""), "cargo");
        assert_eq!(build_style(&[], "./configure --prefix=/usr\nmake", ""), "gnu-configure");
        assert_eq!(build_style(&[], "", "install -Dm755 x"), "");
    }
}
