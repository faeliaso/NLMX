//! The strings the scripts need (`js-*` ids), served as JSON inside the page so `app.js` and
//! `viewer.js` read them through `window.nlmxT(key, args)`.
//!
//! A message with variables (`{ $received }`) is rendered with each variable replaced by the
//! placeholder `{received}`, which the script fills in.

use nlmx_i18n::{Arg, Locale, catalog_source, message_ids, tr_args};
use serde_json::{Map, Value};

/// The variables a message uses: `$name` tokens between its `id =` line and the next message.
fn variables(source: &str, id: &str) -> Vec<String> {
    let header = format!("{id} =");
    let mut found = Vec::new();
    let mut inside = false;
    for line in source.lines() {
        if line.starts_with(|c: char| !c.is_whitespace()) {
            if inside {
                break;
            }
            inside = line.starts_with(&header);
        }
        if !inside {
            continue;
        }
        let mut rest = line;
        while let Some(i) = rest.find('$') {
            let name: String = rest[i + 1..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            if !name.is_empty() && !found.contains(&name) {
                found.push(name);
            }
            rest = &rest[i + 1..];
        }
    }
    found
}

/// Every `js-*` message of `locale` as a JSON object (`{"js-copied": "Copiado."}`).
pub fn catalog(locale: Locale) -> Value {
    let source = catalog_source(locale);
    let mut map = Map::new();
    for id in message_ids(Locale::En)
        .into_iter()
        .filter(|id| id.starts_with("js-"))
    {
        let args: Vec<(String, Arg)> = variables(source, &id)
            .into_iter()
            .map(|name| {
                let placeholder = format!("{{{name}}}");
                (name, Arg::Str(placeholder))
            })
            .collect();
        let refs: Vec<(&str, Arg)> = args.iter().map(|(n, a)| (n.as_str(), a.clone())).collect();
        map.insert(id.clone(), Value::String(tr_args(locale, &id, &refs)));
    }
    Value::Object(map)
}

/// The JSON for the `<script type="application/json" id="i18n-js">` block of the active
/// language. `<` is escaped so the text can never close the script element.
pub fn catalog_json() -> String {
    catalog(nlmx_i18n::current())
        .to_string()
        .replace('<', "\\u003c")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_replace_variables() {
        let pt = catalog(Locale::PtBr);
        assert_eq!(pt["js-copied"], "Copiado.");
        assert_eq!(
            pt["js-download-progress"],
            "{received} de {total} ({percent}%)"
        );
        assert_eq!(catalog(Locale::En)["js-copied"], "Copied.");
    }

    #[test]
    fn json_cannot_close_the_script_element() {
        assert!(!catalog_json().contains("</"));
    }
}
