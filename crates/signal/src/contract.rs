//! The contract between the core and the interfaces: tests
//! keep what the core really sends in `*.example.ts` files, next to the
//! interface's types, and `tsc` checks them against those types when the
//! interface is built: a field missing on either side, or of another type,
//! fails the build. For tests; no new dependency, no generated types.

use std::path::Path;

use serde::Serialize;
use serde_json::Value;

/// Makes `path` hold `examples` of the TypeScript type `ty`, imported from
/// `from`. Panics if it did not already: the core's answer has changed, and the
/// interface's types must be looked at (the file is written: run again).
pub fn check_example<T: Serialize>(path: &str, ty: &str, from: &str, examples: &[T]) {
    let values: Vec<Value> = examples
        .iter()
        .map(|e| serde_json::to_value(e).expect("json"))
        .collect();
    let json = serde_json::to_string_pretty(&values).expect("json");
    let text = format!(
        "// What the core really sends: written by `cargo test` (signal::contract), do not edit.\n\
         // `tsc` checks it against the type: what differs fails the build.\n\
         import type {{ {ty} }} from \"{from}\";\n\n\
         export const examples = {json} satisfies {ty}[];\n"
    );
    let path = Path::new(path);
    let was = std::fs::read_to_string(path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    if was != text {
        std::fs::create_dir_all(path.parent().expect("a folder")).expect("the folder");
        std::fs::write(path, text).expect("the example");
        panic!(
            "{} has changed: look at the interface's types and run again",
            path.display()
        );
    }
}

/// What changes from run to run (times, versions): fixed, wherever these keys are.
pub fn steady(v: &mut Value, keys: &[&str]) {
    match v {
        Value::Array(list) => list.iter_mut().for_each(|e| steady(e, keys)),
        Value::Object(map) => {
            for (k, e) in map.iter_mut() {
                if keys.contains(&k.as_str()) {
                    *e = match e {
                        Value::String(_) => "0".into(),
                        Value::Null => Value::Null,
                        _ => 0.into(),
                    };
                } else {
                    steady(e, keys);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn what_changes_is_fixed_wherever_it_is() {
        let mut v =
            json!({ "now": 5, "a": [{ "at": "x", "keep": 1, "none": null }], "none": null });
        steady(&mut v, &["now", "at", "none"]);
        assert_eq!(
            v,
            json!({ "now": 0, "a": [{ "at": "0", "keep": 1, "none": null }], "none": null })
        );
    }

    #[test]
    fn an_example_that_changed_is_said_and_written() {
        let dir = std::env::temp_dir().join(format!("lumen-contract-{}", std::process::id()));
        let file = dir.join("a.example.ts");
        let path = file.to_str().unwrap();
        let first =
            std::panic::catch_unwind(|| check_example(path, "A", "./a", &[json!({ "x": 1 })]));
        assert!(first.is_err(), "new: said");
        check_example(path, "A", "./a", &[json!({ "x": 1 })]);
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("import type { A } from \"./a\";"), "{text}");
        assert!(text.contains("satisfies A[];"), "{text}");
        let second =
            std::panic::catch_unwind(|| check_example(path, "A", "./a", &[json!({ "x": 2 })]));
        assert!(second.is_err(), "changed: said");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
