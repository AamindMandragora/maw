use serde_json::{json, Value};
use std::{fs, path::Path, process::Command};

// runs nix-instantiate over maw's lib with extra args, returning its output
fn instantiate(args: &[&str], expression: &str) -> std::process::Output {
    let program = format!("let lib = import ./nix/lib.nix; json = builtins.fromJSON; in {expression}");
    Command::new("nix-instantiate")
        .args(["--eval", "--strict", "--json", "-I", "maw=nix"])
        .args(args)
        .args(["-E", &program])
        .output()
        .expect("nix-instantiate on PATH")
}

// evaluates an expression over maw's lib, as json
fn eval(expression: &str) -> Value {
    let output = instantiate(&[], expression);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}

// evaluates an expression that must fail, returning its error text
fn eval_error(expression: &str) -> String {
    let output = instantiate(&[], expression);
    assert!(!output.status.success(), "{expression} evaluated");
    String::from_utf8_lossy(&output.stderr).into_owned()
}

// gvariant strings quote and escape exactly like g_variant_print
#[test]
fn gvariant_matches_dconf_read() {
    let printed = eval(
        r#"map lib.toGVariant [ "it's" ''say "hi"'' ''a'b"c'' ''back\slash'' "a\nb\tc\rd" (json ''"\u001b[0m\u0007\u007f"'') [ ] [ "x" "y" ] true 0.5 (-3) (-1.5) (lib.raw "uint32 5") ]"#,
    );
    let expected = [
        r#""it's""#,
        r#"'say "hi"'"#,
        r#""a'b\"c""#,
        r"'back\\slash'",
        r"'a\nb\tc\rd'",
        r"'\u001b[0m\a\u007f'",
        "@as []",
        "['x', 'y']",
        "true",
        "0.5",
        "-3",
        "-1.5",
        "uint32 5",
    ];
    assert_eq!(printed, json!(expected));
}

// @-blocks inside a rule keep its selector, and comma selectors nest as a cross product
#[test]
fn css_nesting_keeps_selectors() {
    let media = eval(r#"lib.toCSS { ".a" = { color = "red"; "@media (max-width: 1px)" = { color = "blue"; }; }; }"#);
    assert_eq!(media, ".a {\n  color: red;\n}\n\n@media (max-width: 1px) {\n  .a {\n    color: blue;\n  }\n}\n");

    let commas = eval(r#"lib.toCSS { ".a, .b" = { ".c".x = 1; "&:hover, .d".y = 2; ":is(.p, .q)".z = 3; }; }"#);
    let expected = ".a:hover, .b:hover, .a .d, .b .d {\n  y: 2;\n}\n\n.a .c, .b .c {\n  x: 1;\n}\n\n.a :is(.p, .q), .b :is(.p, .q) {\n  z: 3;\n}\n";
    assert_eq!(commas, expected);

    let font_face = eval(r#"lib.toCSS { "@font-face" = { font-family = "x"; src = [ "a" "b" ]; }; }"#);
    assert_eq!(font_face, "@font-face {\n  font-family: x;\n  src: a, b;\n}\n");
}

// kdl strings use kdl escapes, property names quote like node names, mixed lists fail
#[test]
fn kdl_escapes_and_names() {
    let rendered = eval(r#"lib.toKDL { n = lib.kdl.node [ (json ''"a\u001b\"\\\n"'') ] { "a b" = 1; ok = "x"; } { }; "my node" = "v"; }"#);
    assert_eq!(rendered, "\"my node\" \"v\"\nn \"a\\u{1b}\\\"\\\\\\n\" \"a b\"=1 ok=\"x\"\n");
    assert!(eval_error(r#"lib.toKDL { n = [ 1 { a = 1; } ]; }"#).contains("maw: toKDL node n mixes values and blocks in one list"));
}

// toml rejects null by key and escapes del
#[test]
fn toml_null_and_del() {
    assert!(eval_error("lib.toTOML { t.a = null; }").contains("maw: toTOML has no null (key a); leave the key out"));
    assert_eq!(eval(r#"lib.toTOML { a = json ''"x\u007f"''; }"#), "a = \"x\\u007F\"\n");
}

// paths render as their literal text instead of being copied to the store
#[test]
fn paths_stay_literal() {
    let rendered = eval(r#"map (f: f { p = /no/such/file; }) [ lib.toJSON lib.toTOML lib.toKDL ]"#);
    assert_eq!(rendered, json!(["{\n  \"p\": \"/no/such/file\"\n}\n", "p = \"/no/such/file\"\n", "p \"/no/such/file\"\n"]));
}

// a service's scope must be system or user
#[test]
fn service_rejects_unknown_scope() {
    assert!(eval_error(r#"lib.service "x" { scope = "usr"; run = "x"; }"#).contains("maw: service x has scope usr"));
    assert_eq!(eval(r#"(builtins.head (lib.service "x" { scope = "user"; run = "x"; })).scope"#), "user");
}

// config.nix and hosts/<name>.nix get onHosts, and the host file merges over config
#[test]
fn host_config_merges_with_on_hosts() {
    let dotfiles = tempfile::tempdir().unwrap();
    let root = dotfiles.path();
    let write = |path: &str, text: &str| {
        fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
        fs::write(root.join(path), text).unwrap();
    };
    write("config.nix", r#"{ lib, ... }: { a = { x = 1; y = 2; }; list = [ 1 2 ]; here = lib.onHosts [ "box" ] "yes"; }"#);
    write("hosts/box.nix", r#"{ lib, ... }: { a.y = 3; list = [ 9 ]; there = lib.onHosts [ "other" ] "no"; }"#);
    write("maw.nix", "{ }");
    write("modules/probe.nix", r#"{ config, ... }: [ { name = "probe"; key = "main"; content = builtins.toJSON config; } ]"#);
    write("state/host", "box\n");

    let config_dir = format!("maw-config={}", root.join("state").display());
    let expression = format!("(builtins.head (import ./nix/default.nix {{ dotfiles = {}; }}).modules.probe).content", root.display());
    let output = instantiate(&["-I", &config_dir], &expression);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    let content: String = serde_json::from_slice(&output.stdout).unwrap();
    let config: Value = serde_json::from_str(&content).unwrap();
    assert_eq!(config, json!({ "a": { "x": 1, "y": 3 }, "list": [9], "here": "yes", "there": [] }));
}

// the scaffolder reads empty homepages, and meson wins over cargoDeps
#[test]
fn nixpkgs_meta_handles_meson_rust_and_empty_homepage() {
    let nixpkgs = tempfile::tempdir().unwrap();
    let fake = r#"{ ... }: {
      lib = import <maw/nixpkgs-lib>;
      app = { type = "derivation"; pname = "app"; cargoDeps = { }; meta.homepage = [ ];
        nativeBuildInputs = [ { type = "derivation"; pname = "meson"; } ]; };
    }"#;
    fs::write(nixpkgs.path().join("default.nix"), fake).unwrap();

    let meta = Path::new("nix/nixpkgs-meta.nix").canonicalize().unwrap();
    let expression = format!("import {} {{ nixpkgs = {}; attr = \"app\"; }}", meta.display(), nixpkgs.path().display());
    let result = eval(&expression);
    assert_eq!((result["builder"].as_str(), result["homepage"].as_str()), (Some("meson"), Some("")));
}
