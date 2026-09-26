use crate::activate::Step;
use crate::runner::{RunError, Runner};
use std::collections::BTreeMap;

// desktop settings in dconf, written with dconf write and tracked like files: maw remembers what it last wrote,
// so a value changed by hand since is reported instead of overwritten

// the value maw last wrote to each key
pub type Record = BTreeMap<String, String>;

// one key and its GVariant value
type Setting = (String, String);

fn dconf(runner: &dyn Runner, args: &[&str]) -> Result<String, RunError> {
    runner.run("dconf", &args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>())
}

// every declared key read back; dconf prints nothing for a key that isn't set
fn read_all(runner: &dyn Runner, keys: impl Iterator<Item = String>) -> Result<BTreeMap<String, String>, RunError> {
    keys.map(|key| Ok((key.clone(), dconf(runner, &["read", &key])?.trim().to_string()))).collect()
}

// the steps that bring dconf to what's declared, and the record they leave; nothing is read or written when nothing is
// declared. dconf reads its database directly, but writing goes through the session bus, so without one nothing is planned
pub fn plan(runner: &dyn Runner, session_bus: bool, wanted: &BTreeMap<String, String>, record: &Record, force: bool) -> (Vec<Step>, Record) {
    if wanted.is_empty() && record.is_empty() {
        return (Vec::new(), Record::new());
    }
    if !session_bus {
        return (vec![Step::SettingsSkipped { reason: "dconf needs a desktop session".into() }], record.clone());
    }
    let live = match read_all(runner, wanted.keys().chain(record.keys()).cloned()) {
        Ok(live) => live,
        Err(RunError::Spawn { .. }) => return (vec![Step::SettingsSkipped { reason: "dconf isn't installed".into() }], record.clone()),
        Err(_) => return (vec![Step::SettingsSkipped { reason: "dconf needs a desktop session".into() }], record.clone()),
    };
    let now = |key: &String| live.get(key).cloned().unwrap_or_default();

    // declared keys: in place already, changed by hand since maw wrote them, or to write
    let declared = wanted.iter().map(|(key, value)| match record.get(key) {
        _ if now(key) == *value => (None, Some((key.clone(), value.clone()))),
        Some(last) if now(key) != *last && !force => (Some(Step::SettingEdited { key: key.clone() }), Some((key.clone(), last.clone()))),
        _ => (Some(Step::Setting { key: key.clone(), value: value.clone() }), Some((key.clone(), value.clone()))),
    });

    // keys nothing declares anymore are reset while they still hold what maw wrote, and forgotten either way
    let dropped = record.iter().filter(|(key, _)| !wanted.contains_key(*key)).map(|(key, last)| ((now(key) == *last).then(|| Step::SettingReset { key: key.clone() }), None));

    let (steps, next): (Vec<Option<Step>>, Vec<Option<Setting>>) = declared.chain(dropped).unzip();
    (steps.into_iter().flatten().collect(), next.into_iter().flatten().collect())
}

// writes and resets the planned keys
pub fn apply(runner: &dyn Runner, steps: &[Step]) -> Result<(), RunError> {
    steps.iter().try_for_each(|step| match step {
        Step::Setting { key, value } => dconf(runner, &["write", key, value]).map(drop),
        Step::SettingReset { key } => dconf(runner, &["reset", key]).map(drop),
        _ => Ok(()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::FakeRunner;

    const DARK: &str = "/org/gnome/desktop/interface/color-scheme";
    const FONT: &str = "/org/gnome/desktop/interface/font-name";

    // dconf answering reads from a fixed database
    fn dconf_with(values: &[(&str, &str)]) -> FakeRunner {
        let values: BTreeMap<String, String> = values.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect();
        FakeRunner::new(move |_, args| match args[0].as_str() {
            "read" => values.get(&args[1]).map(|value| format!("{value}\n")).unwrap_or_default(),
            _ => String::new(),
        })
    }

    fn wanted(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect()
    }

    #[test]
    fn unset_and_changed_keys_are_written_and_recorded() {
        let runner = dconf_with(&[(DARK, "'default'")]);
        let (steps, record) = plan(&runner, true, &wanted(&[(DARK, "'prefer-dark'"), (FONT, "'Adwaita Sans 11'")]), &Record::new(), false);
        assert_eq!(steps, [Step::Setting { key: DARK.into(), value: "'prefer-dark'".into() }, Step::Setting { key: FONT.into(), value: "'Adwaita Sans 11'".into() }]);
        assert_eq!(record, wanted(&[(DARK, "'prefer-dark'"), (FONT, "'Adwaita Sans 11'")]));

        apply(&runner, &steps).unwrap();
        assert!(runner.calls.borrow().contains(&format!("dconf write {DARK} 'prefer-dark'")));
    }

    #[test]
    fn values_already_in_place_need_nothing() {
        let runner = dconf_with(&[(DARK, "'prefer-dark'")]);
        let record = wanted(&[(DARK, "'prefer-dark'")]);
        assert_eq!(plan(&runner, true, &record, &record, false), (Vec::new(), record.clone()));
    }

    #[test]
    fn hand_changes_are_reported_unless_forced() {
        let runner = dconf_with(&[(DARK, "'prefer-light'")]);
        let record = wanted(&[(DARK, "'prefer-dark'")]);
        let (steps, kept) = plan(&runner, true, &record, &record, false);
        assert_eq!((steps, kept), (vec![Step::SettingEdited { key: DARK.into() }], record.clone()));
        assert_eq!(plan(&runner, true, &record, &record, true).0, [Step::Setting { key: DARK.into(), value: "'prefer-dark'".into() }]);
    }

    #[test]
    fn undeclared_keys_are_reset_only_while_still_ours() {
        let record = wanted(&[(DARK, "'prefer-dark'"), (FONT, "'Adwaita Sans 11'")]);
        let runner = dconf_with(&[(DARK, "'prefer-dark'"), (FONT, "'Inter 11'")]);
        let (steps, next) = plan(&runner, true, &BTreeMap::new(), &record, false);
        assert_eq!((steps, next), (vec![Step::SettingReset { key: DARK.into() }], Record::new()));
    }

    #[test]
    fn without_a_session_bus_nothing_is_read_or_written() {
        let runner = dconf_with(&[]);
        let (steps, _) = plan(&runner, false, &wanted(&[(DARK, "'prefer-dark'")]), &Record::new(), false);
        assert_eq!(steps, [Step::SettingsSkipped { reason: "dconf needs a desktop session".into() }]);
        assert!(runner.calls.borrow().is_empty());
    }

    #[test]
    fn without_dconf_settings_are_skipped() {
        let runner = FakeRunner::fallible(|_, _| Err(RunError::Failed { program: "dconf".into(), stderr: "no bus".into() }));
        let (steps, record) = plan(&runner, true, &wanted(&[(DARK, "'prefer-dark'")]), &Record::new(), false);
        assert_eq!((steps, record), (vec![Step::SettingsSkipped { reason: "dconf needs a desktop session".into() }], Record::new()));
    }
}
