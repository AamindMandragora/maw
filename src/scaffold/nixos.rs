use super::ScaffoldError;
use crate::backend::Pkg;
use crate::env::Env;
use crate::runner::Runner;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;

// search.nixos.org's package search: the elasticsearch backend its own page queries. It isn't a documented api,
// so where it lives and its read-only login are read from the page's script and cached, and read again when a query fails
const SITE: &str = "https://search.nixos.org";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Backend {
    index: String,
    login: String,
}

fn cache_file(env: &Env) -> PathBuf {
    env.cache_dir().join("nixos-search.json")
}

// a quoted value after a key in the page's script: elasticsearchUsername:"abc" -> abc
fn flag<'s>(script: &'s str, key: &str) -> Option<&'s str> {
    let marker = format!("{key}:");
    let rest = &script[script.find(&marker)? + marker.len()..];
    let rest = rest.trim_start_matches([':', ' ']).trim_start_matches("parseInt(");
    let rest = rest.strip_prefix('"')?;
    Some(&rest[..rest.find('"')?])
}

// the backend's index and login, from the flags the page's script starts with
fn scrape(runner: &dyn Runner) -> Option<Backend> {
    let page = runner.run("curl", &["-fsSL".into(), format!("{SITE}/packages")]).ok()?;
    let script_path = page.split("src=\"").skip(1).map(|rest| &rest[..rest.find('"').unwrap_or(0)]).find(|path| path.contains("/index.") && path.ends_with(".js"))?;
    let script = runner.run("curl", &["-fsSL".into(), format!("{SITE}{script_path}")]).ok()?;
    let version = flag(&script, "elasticsearchMappingSchemaVersion")?;
    let login = format!("{}:{}", flag(&script, "elasticsearchUsername")?, flag(&script, "elasticsearchPassword")?);
    Some(Backend { index: format!("latest-{version}-nixos-unstable"), login })
}

// packages matching a term, best first: name matches outrank description ones
fn query(runner: &dyn Runner, backend: &Backend, term: &str) -> Option<Vec<Pkg>> {
    let body = json!({
        "size": 10,
        "_source": ["package_attr_name", "package_pversion", "package_description", "package_homepage"],
        "query": { "bool": {
            "filter": [{ "term": { "type": "package" } }],
            "must": [{ "multi_match": { "query": term, "fields": ["package_attr_name^9", "package_pname^6", "package_description"] } }],
        } },
    });
    let url = format!("{SITE}/backend/{}/_search", backend.index);
    let args = ["-fsS", "-u", &backend.login, "-H", "Content-Type: application/json", &url, "-d", &body.to_string()].map(String::from);
    let response: Value = serde_json::from_str(&runner.run("curl", &args).ok()?).ok()?;
    let hits = response["hits"]["hits"].as_array()?;
    let field = |hit: &Value, key: &str| hit["_source"][key].as_str().unwrap_or_default().to_string();
    let homepage = |hit: &Value| hit["_source"]["package_homepage"].get(0).and_then(Value::as_str).unwrap_or_default().to_string();
    Some(hits.iter().map(|hit| Pkg { name: field(hit, "package_attr_name"), source: field(hit, "package_attr_name"), version: field(hit, "package_pversion"), description: field(hit, "package_description"), homepage: homepage(hit), ..Pkg::default() }).collect())
}

// nixpkgs packages matching a term, through the cached backend, found again once if it stopped answering
pub fn search(env: &Env, runner: &dyn Runner, term: &str) -> Result<Vec<Pkg>, ScaffoldError> {
    let cached: Option<Backend> = fs::read_to_string(cache_file(env)).ok().and_then(|text| serde_json::from_str(&text).ok());
    if let Some(found) = cached.as_ref().and_then(|backend| query(runner, backend, term)) {
        return Ok(found);
    }
    let backend = scrape(runner).ok_or(ScaffoldError::NixSearch)?;
    let found = query(runner, &backend, term).ok_or(ScaffoldError::NixSearch)?;
    fs::create_dir_all(env.cache_dir()).ok();
    fs::write(cache_file(env), serde_json::to_string(&backend).unwrap_or_default()).ok();
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::FakeRunner;
    use std::path::Path;

    const PAGE: &str = r#"<script defer src="/static/js/779.0ab060af.js"></script><script defer src="/static/js/index.771b9a4a.js"></script>"#;
    const SCRIPT: &str = r#"init({flags:{elasticsearchMappingSchemaVersion:parseInt("51"),elasticsearchUrl:"/backend",elasticsearchUsername:"user",elasticsearchPassword:"pass",nixosChannels"#;
    const HITS: &str = r#"{"hits":{"hits":[{"_source":{"package_attr_name":"wayfreeze","package_pversion":"0.2.1","package_description":"freeze the screen","package_homepage":["https://github.com/Jappie3/wayfreeze"]}}]}}"#;

    fn fake(_: &str, args: &[String]) -> String {
        match args.iter().find(|arg| arg.starts_with("https://")).map(String::as_str) {
            Some(url) if url.ends_with("/packages") => PAGE.into(),
            Some(url) if url.ends_with(".js") => SCRIPT.into(),
            Some(url) if url.contains("/backend/latest-51-nixos-unstable/") => HITS.into(),
            _ => String::new(),
        }
    }

    #[test]
    fn the_backend_comes_from_the_page_script() {
        let runner = FakeRunner::new(fake);
        assert_eq!(scrape(&runner), Some(Backend { index: "latest-51-nixos-unstable".into(), login: "user:pass".into() }));
    }

    #[test]
    fn search_finds_packages_and_caches_the_backend() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::new(dir.path(), Path::new("/"), Path::new("/s"));
        let runner = FakeRunner::new(fake);
        let found = search(&env, &runner, "wayfreeze").unwrap();
        assert_eq!((found[0].name.as_str(), found[0].version.as_str(), found[0].homepage.as_str()), ("wayfreeze", "0.2.1", "https://github.com/Jappie3/wayfreeze"));

        // a second search goes straight to the cached backend
        let before = runner.calls.borrow().len();
        search(&env, &runner, "wayfreeze").unwrap();
        assert_eq!(runner.calls.borrow().len(), before + 1);
    }

    #[test]
    fn a_changed_site_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::new(dir.path(), Path::new("/"), Path::new("/s"));
        assert!(matches!(search(&env, &FakeRunner::new(|_, _| String::new()), "x"), Err(ScaffoldError::NixSearch)));
    }
}
