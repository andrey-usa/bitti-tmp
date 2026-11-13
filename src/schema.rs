//! `expect json [<$.path>] schema <file.json>`: JSON Schema, the parts an
//! API's contract uses (drafts 4 to 2020-12 read alike here): `type`
//! (`nullable` from OpenAPI 3.0 too), `enum`, `const`, `properties`,
//! `required`, `additionalProperties`, `patternProperties`,
//! `propertyNames`, `minProperties`/`maxProperties`, `dependentRequired`,
//! `items` (a schema, or a list for each place), `prefixItems`,
//! `contains`, `minItems`/`maxItems`, `uniqueItems`, `minLength`/
//! `maxLength`, `pattern`, `format` (`date-time`, `date`, `time`, `email`,
//! `uuid`, `uri`, `ipv4`, `ipv6`), `minimum`/`maximum` and their exclusive
//! forms (a number or, in draft 4, a flag), `multipleOf`, `allOf`,
//! `anyOf`, `oneOf`, `not`, `if`/`then`/`else`, and `$ref` within the file
//! (`#/$defs/…`, `#/definitions/…`, `#`, an OpenAPI document's
//! `#/components/schemas/…`). Anything else in a schema is
//! ignored, as validators do with keywords they don't know.

use std::cell::RefCell;
use std::collections::HashMap;

use serde_json::{Map, Value};

/// The most errors a failure lists: the first ones say what is wrong.
const MAX_ERRORS: usize = 8;

/// Ok, or what doesn't hold, each as `<$.path>: <what>`.
#[cfg(test)]
pub fn validate(schema: &Value, doc: &Value) -> Result<(), Vec<String>> {
    validate_in(schema, schema, doc)
}

/// [`validate`] with a schema that is part of `root` (an OpenAPI
/// document's `#/components/schemas/Item`): its `$ref`s are read in `root`.
pub fn validate_in(root: &Value, schema: &Value, doc: &Value) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    check(schema, doc, "$", root, &mut errors, 0);
    if errors.is_empty() {
        Ok(())
    } else {
        let n = errors.len();
        errors.truncate(MAX_ERRORS);
        if n > MAX_ERRORS {
            errors.push(format!("… {} more", n - MAX_ERRORS));
        }
        Err(errors)
    }
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() || n.as_f64().is_some_and(|f| f.fract() == 0.0) => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// A value as a failure shows it: short.
fn shown(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 60 {
        format!("{}…", s.chars().take(60).collect::<String>())
    } else {
        s
    }
}

fn is_type(v: &Value, t: &str) -> bool {
    match t {
        "number" => v.is_number(),
        "integer" => kind(v) == "integer",
        other => kind(v) == other,
    }
}

fn equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| equal(p, q)),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| equal(v, w)))
        }
        _ => a == b,
    }
}

/// A value written so that two values are [`equal`] exactly when they
/// are written the same: numbers by their `f64` (0 and -0 alike), object
/// keys sorted, strings and numbers delimited.
fn canonical(v: &Value, out: &mut String) {
    use std::fmt::Write as _;
    match v {
        Value::Null => out.push('n'),
        Value::Bool(b) => out.push(if *b { 't' } else { 'f' }),
        Value::Number(n) => match n.as_f64() {
            Some(x) => {
                let x = if x == 0.0 { 0.0 } else { x };
                let _ = write!(out, "d{:x};", x.to_bits());
            }
            None => out.push_str("d;"),
        },
        Value::String(t) => {
            let _ = write!(out, "s{}:", t.len());
            out.push_str(t);
        }
        Value::Array(items) => {
            out.push('[');
            for item in items {
                canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort_unstable();
            out.push('{');
            for k in keys {
                let _ = write!(out, "s{}:", k.len());
                out.push_str(k);
                canonical(&m[k], out);
            }
            out.push('}');
        }
    }
}

/// `uniqueItems`: the first item that comes again later, and where it
/// first does (as comparing every pair in order finds them), by one key
/// per item: comparing pairs is quadratic in a long list.
fn first_repeat(items: &[Value]) -> Option<(usize, usize)> {
    let mut first: HashMap<String, (usize, bool)> = HashMap::with_capacity(items.len());
    let mut found: Option<(usize, usize)> = None;
    let mut key = String::new();
    for (j, item) in items.iter().enumerate() {
        key.clear();
        canonical(item, &mut key);
        match first.get_mut(key.as_str()) {
            Some((i, again)) => {
                if !*again {
                    *again = true;
                    if found.is_none_or(|(f, _)| *i < f) {
                        found = Some((*i, j));
                    }
                }
            }
            None => {
                first.insert(key.clone(), (j, false));
            }
        }
    }
    found
}

/// `#/$defs/user` (or `#`) in the root schema.
fn resolve<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    let pointer = reference.strip_prefix('#')?;
    if pointer.is_empty() {
        return Some(root);
    }
    // JSON pointer escapes, and percent-encoded characters a ref may hold.
    let decoded = pointer.replace("%25", "%").replace("%22", "\"");
    root.pointer(&decoded)
}

fn key_path(at: &str, key: &str) -> String {
    if !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        format!("{at}.{key}")
    } else {
        format!("{at}[{}]", Value::String(key.to_string()))
    }
}

/// Whether `s` matches the regular expression `p`, compiled once per
/// thread: a schema's `pattern`s and `format`s are checked against every
/// value of a long list, and compiling one costs more than checking a
/// thousand values with it.
fn is_match(p: &str, s: &str) -> Result<bool, regex::Error> {
    thread_local! {
        static COMPILED: RefCell<HashMap<String, Result<regex::Regex, regex::Error>>> = RefCell::default();
    }
    COMPILED.with(|compiled| {
        let mut compiled = compiled.borrow_mut();
        if !compiled.contains_key(p) {
            compiled.insert(p.to_string(), regex::Regex::new(p));
        }
        match &compiled[p] {
            Ok(re) => Ok(re.is_match(s)),
            Err(e) => Err(e.clone()),
        }
    })
}

fn format_ok(format: &str, s: &str) -> bool {
    let re = |p: &str| is_match(p, s).unwrap_or(false);
    match format {
        "date-time" => re(r"^\d{4}-\d{2}-\d{2}[Tt ]\d{2}:\d{2}:\d{2}(\.\d+)?([Zz]|[+-]\d{2}:\d{2})$"),
        "date" => re(r"^\d{4}-(0[1-9]|1[0-2])-(0[1-9]|[12]\d|3[01])$"),
        "time" => re(r"^\d{2}:\d{2}:\d{2}(\.\d+)?([Zz]|[+-]\d{2}:\d{2})?$"),
        "email" => re(r"^[^@\s]+@[^@\s]+\.[^@\s]+$"),
        "uuid" => re(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"),
        "uri" => re(r"^[A-Za-z][A-Za-z0-9+.-]*:[^\s]*$"),
        "ipv4" => s.parse::<std::net::Ipv4Addr>().is_ok(),
        "ipv6" => s.parse::<std::net::Ipv6Addr>().is_ok(),
        // A format this doesn't know is an annotation, not a check.
        _ => true,
    }
}

fn check(schema: &Value, v: &Value, at: &str, root: &Value, errors: &mut Vec<String>, depth: usize) {
    if depth > 64 {
        errors.push(format!("{at}: the schema's $refs go more than 64 deep (a loop?)"));
        return;
    }
    let s: &Map<String, Value> = match schema {
        Value::Bool(true) => return,
        Value::Bool(false) => {
            errors.push(format!("{at}: nothing is allowed here (the schema is false), got {}", shown(v)));
            return;
        }
        Value::Object(m) => m,
        _ => return,
    };
    if let Some(r) = s.get("$ref").and_then(Value::as_str) {
        match resolve(root, r) {
            Some(target) => check(target, v, at, root, errors, depth + 1),
            None => errors.push(format!("{at}: $ref {r} names nothing in the schema (only refs within the file)")),
        }
        // Draft 2019+ reads the keywords beside a $ref too; older ones
        // don't, and those schemas seldom have any.
    }
    let mut err_local: Vec<String> = Vec::new();
    let mut push = |m: String| err_local.push(format!("{at}: {m}"));
    // type
    if !(v.is_null() && s.get("nullable").and_then(Value::as_bool) == Some(true)) {
        match s.get("type") {
            Some(Value::String(t)) if !is_type(v, t) => push(format!("expected {t}, got {} {}", kind(v), shown(v))),
            Some(Value::Array(ts)) if !ts.iter().filter_map(Value::as_str).any(|t| is_type(v, t)) => {
                let names: Vec<&str> = ts.iter().filter_map(Value::as_str).collect();
                push(format!("expected {}, got {} {}", names.join(" or "), kind(v), shown(v)))
            }
            _ => {}
        }
    } else if v.is_null() {
        // `nullable: true` and a null: nothing more to check.
        errors.append(&mut err_local);
        return;
    }
    if let Some(Value::Array(options)) = s.get("enum") {
        if !options.iter().any(|o| equal(o, v)) {
            let list: Vec<String> = options.iter().map(shown).collect();
            push(format!("expected one of {}, got {}", list.join(", "), shown(v)));
        }
    }
    if let Some(c) = s.get("const") {
        if !equal(c, v) {
            push(format!("expected {}, got {}", shown(c), shown(v)));
        }
    }
    errors.append(&mut err_local);
    let mut err2 = |m: String| errors.push(format!("{at}: {m}"));
    match v {
        Value::String(text) => {
            let len = text.chars().count() as u64;
            if let Some(n) = s.get("minLength").and_then(Value::as_u64) {
                if len < n {
                    err2(format!("expected at least {n} characters, got {len} ({})", shown(v)));
                }
            }
            if let Some(n) = s.get("maxLength").and_then(Value::as_u64) {
                if len > n {
                    err2(format!("expected at most {n} characters, got {len} ({})", shown(v)));
                }
            }
            if let Some(p) = s.get("pattern").and_then(Value::as_str) {
                match is_match(p, text) {
                    Ok(false) => err2(format!("{} doesn't match the pattern {p:?}", shown(v))),
                    Err(e) => err2(format!("the schema's pattern {p:?} isn't a regular expression here ({e})")),
                    _ => {}
                }
            }
            if let Some(f) = s.get("format").and_then(Value::as_str) {
                if !format_ok(f, text) {
                    err2(format!("{} isn't a {f}", shown(v)));
                }
            }
        }
        Value::Number(n) => {
            let x = n.as_f64().unwrap_or(f64::NAN);
            let num = |k: &str| s.get(k).and_then(Value::as_f64);
            let flag = |k: &str| s.get(k).and_then(Value::as_bool).unwrap_or(false);
            if let Some(m) = num("minimum") {
                // Draft 4: `exclusiveMinimum: true` makes the minimum exclusive.
                if (flag("exclusiveMinimum") && x <= m) || x < m {
                    err2(format!("expected {} {m}, got {x}", if flag("exclusiveMinimum") { ">" } else { ">=" }));
                }
            }
            if let Some(m) = num("maximum") {
                if (flag("exclusiveMaximum") && x >= m) || x > m {
                    err2(format!("expected {} {m}, got {x}", if flag("exclusiveMaximum") { "<" } else { "<=" }));
                }
            }
            if let Some(m) = num("exclusiveMinimum") {
                if x <= m {
                    err2(format!("expected > {m}, got {x}"));
                }
            }
            if let Some(m) = num("exclusiveMaximum") {
                if x >= m {
                    err2(format!("expected < {m}, got {x}"));
                }
            }
            if let Some(m) = num("multipleOf").filter(|m| *m > 0.0) {
                let q = x / m;
                if (q - q.round()).abs() > 1e-9 {
                    err2(format!("expected a multiple of {m}, got {x}"));
                }
            }
        }
        _ => {}
    }
    if let Value::Array(items) = v {
        if let Some(n) = s.get("minItems").and_then(Value::as_u64) {
            if (items.len() as u64) < n {
                err2(format!("expected at least {n} items, got {}", items.len()));
            }
        }
        if let Some(n) = s.get("maxItems").and_then(Value::as_u64) {
            if items.len() as u64 > n {
                err2(format!("expected at most {n} items, got {}", items.len()));
            }
        }
        if s.get("uniqueItems").and_then(Value::as_bool) == Some(true) {
            if let Some((i, j)) = first_repeat(items) {
                err2(format!("items {i} and {j} are the same ({}), and they must be unique", shown(&items[i])));
            }
        }
        // `prefixItems` (2020-12) or `items` as a list (older): one schema per place.
        let (places, rest) = match (s.get("prefixItems"), s.get("items")) {
            (Some(Value::Array(p)), rest) => (Some(p), rest),
            (None, Some(Value::Array(p))) => (Some(p), s.get("additionalItems")),
            (_, rest) => (None, rest),
        };
        let skip = places.map_or(0, |p| p.len());
        if let Some(places) = places {
            for (i, (sub, item)) in places.iter().zip(items).enumerate() {
                check(sub, item, &format!("{at}[{i}]"), root, errors, depth + 1);
            }
        }
        if let Some(sub) = rest.filter(|r| !r.is_array()) {
            for (i, item) in items.iter().enumerate().skip(skip) {
                check(sub, item, &format!("{at}[{i}]"), root, errors, depth + 1);
            }
        }
        if let Some(sub) = s.get("contains") {
            let hits = items.iter().filter(|item| ok(sub, item, root, depth)).count() as u64;
            let min = s.get("minContains").and_then(Value::as_u64).unwrap_or(1);
            let max = s.get("maxContains").and_then(Value::as_u64);
            if hits < min || max.is_some_and(|m| hits > m) {
                errors.push(format!("{at}: expected an item that matches `contains` ({hits} of {} do)", items.len()));
            }
        }
    }
    if let Value::Object(obj) = v {
        let mut err3 = |m: String| errors.push(format!("{at}: {m}"));
        if let Some(Value::Array(req)) = s.get("required") {
            let missing: Vec<&str> = req.iter().filter_map(Value::as_str).filter(|k| !obj.contains_key(*k)).collect();
            if !missing.is_empty() {
                err3(format!("missing required {}", missing.join(", ")));
            }
        }
        if let Some(n) = s.get("minProperties").and_then(Value::as_u64) {
            if (obj.len() as u64) < n {
                err3(format!("expected at least {n} properties, got {}", obj.len()));
            }
        }
        if let Some(n) = s.get("maxProperties").and_then(Value::as_u64) {
            if obj.len() as u64 > n {
                err3(format!("expected at most {n} properties, got {}", obj.len()));
            }
        }
        if let Some(Value::Object(deps)) = s.get("dependentRequired") {
            for (k, needs) in deps {
                if obj.contains_key(k) {
                    for n in needs.as_array().into_iter().flatten().filter_map(Value::as_str) {
                        if !obj.contains_key(n) {
                            err3(format!("{k} is there, so {n} must be too"));
                        }
                    }
                }
            }
        }
        let props = s.get("properties").and_then(Value::as_object);
        let patterns: Vec<(&str, &Value)> = s
            .get("patternProperties")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(p, sub)| (p.as_str(), sub))
            .collect();
        for (k, item) in obj {
            let here = key_path(at, k);
            let mut known = false;
            if let Some(sub) = props.and_then(|p| p.get(k)) {
                known = true;
                check(sub, item, &here, root, errors, depth + 1);
            }
            for (p, sub) in &patterns {
                if is_match(p, k).unwrap_or(false) {
                    known = true;
                    check(sub, item, &here, root, errors, depth + 1);
                }
            }
            if !known {
                match s.get("additionalProperties") {
                    Some(Value::Bool(false)) => {
                        errors.push(format!("{at}: {k:?} isn't allowed (additionalProperties: false)"))
                    }
                    Some(sub @ Value::Object(_)) => check(sub, item, &here, root, errors, depth + 1),
                    _ => {}
                }
            }
            if let Some(names) = s.get("propertyNames") {
                if !ok(names, &Value::String(k.clone()), root, depth) {
                    errors.push(format!("{at}: the property name {k:?} doesn't match propertyNames"));
                }
            }
        }
    }
    if let Some(Value::Array(all)) = s.get("allOf") {
        for sub in all {
            check(sub, v, at, root, errors, depth + 1);
        }
    }
    if let Some(Value::Array(any)) = s.get("anyOf") {
        if !any.iter().any(|sub| ok(sub, v, root, depth)) {
            // The closest option's errors say most.
            let best = any.iter().map(|sub| collect(sub, v, at, root, depth)).min_by_key(Vec::len).unwrap_or_default();
            errors.push(format!("{at}: matches none of anyOf's {} options", any.len()));
            errors.extend(best);
        }
    }
    if let Some(Value::Array(one)) = s.get("oneOf") {
        let n = one.iter().filter(|sub| ok(sub, v, root, depth)).count();
        if n != 1 {
            errors.push(format!("{at}: expected exactly one of oneOf's {} options to match, {n} do", one.len()));
        }
    }
    if let Some(sub) = s.get("not") {
        if ok(sub, v, root, depth) {
            errors.push(format!("{at}: matches the schema under `not`, got {}", shown(v)));
        }
    }
    if let Some(cond) = s.get("if") {
        let branch = if ok(cond, v, root, depth) { s.get("then") } else { s.get("else") };
        if let Some(b) = branch {
            check(b, v, at, root, errors, depth + 1);
        }
    }
}

fn collect(schema: &Value, v: &Value, at: &str, root: &Value, depth: usize) -> Vec<String> {
    let mut e = Vec::new();
    check(schema, v, at, root, &mut e, depth + 1);
    e
}

fn ok(schema: &Value, v: &Value, root: &Value, depth: usize) -> bool {
    collect(schema, v, "$", root, depth).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_contract_holds_or_says_where_it_doesnt() {
        let schema = json!({
            "type": "object",
            "required": ["id", "email", "role"],
            "properties": {
                "id": {"type": "integer", "minimum": 1},
                "email": {"type": "string", "format": "email"},
                "role": {"enum": ["shopper", "admin"]},
                "tags": {"type": "array", "items": {"type": "string"}, "uniqueItems": true},
                "manager": {"$ref": "#/$defs/ref"}
            },
            "additionalProperties": false,
            "$defs": {"ref": {"type": ["integer", "null"]}}
        });
        assert!(validate(
            &schema,
            &json!({"id": 7, "email": "a@b.co", "role": "admin", "tags": ["x"], "manager": null})
        )
        .is_ok());
        let errs = validate(
            &schema,
            &json!({"id": "7", "email": "nope", "role": "boss", "tags": ["x", "x"], "extra": 1, "manager": "m"}),
        )
        .unwrap_err();
        let all = errs.join("\n");
        assert!(all.contains("$.id: expected integer, got string \"7\""), "{all}");
        assert!(all.contains("$.email: \"nope\" isn't a email"), "{all}");
        assert!(all.contains("$.role: expected one of \"shopper\", \"admin\", got \"boss\""), "{all}");
        assert!(all.contains("$.tags: items 0 and 1 are the same"), "{all}");
        assert!(all.contains("$: \"extra\" isn't allowed"), "{all}");
        assert!(all.contains("$.manager: expected integer or null"), "{all}");
        let errs = validate(&schema, &json!({"id": 0})).unwrap_err();
        assert!(errs.iter().any(|e| e == "$: missing required email, role"), "{errs:?}");
        assert!(errs.iter().any(|e| e == "$.id: expected >= 1, got 0"), "{errs:?}");
    }

    #[test]
    fn arrays_numbers_and_combinators() {
        let list = json!({"type": "array", "minItems": 1, "items": {"type": "object", "required": ["sku"], "properties": {"price": {"type": "number", "exclusiveMinimum": 0, "multipleOf": 0.01}}}});
        assert!(validate(&list, &json!([{"sku": "a", "price": 9.99}])).is_ok());
        let errs = validate(&list, &json!([{"sku": "a", "price": 0}, {"price": 1.005}])).unwrap_err().join("\n");
        assert!(errs.contains("$[0].price: expected > 0, got 0"), "{errs}");
        assert!(errs.contains("$[1]: missing required sku"), "{errs}");
        assert!(errs.contains("$[1].price: expected a multiple of 0.01"), "{errs}");
        assert!(validate(&list, &json!([])).unwrap_err()[0].contains("at least 1 items"));
        let one = json!({"oneOf": [{"type": "string"}, {"type": "integer"}]});
        assert!(validate(&one, &json!(3)).is_ok());
        assert!(validate(&one, &json!(true)).is_err());
        let any = json!({"anyOf": [{"type": "string", "minLength": 3}, {"type": "null"}]});
        let e = validate(&any, &json!("ab")).unwrap_err();
        assert!(e[0].contains("none of anyOf's 2 options"), "{e:?}");
        assert!(e.iter().any(|m| m.contains("at least 3 characters")), "{e:?}");
        assert!(validate(&json!({"not": {"type": "null"}}), &json!(null)).is_err());
        let tuple = json!({"prefixItems": [{"type": "string"}, {"type": "integer"}], "items": false});
        assert!(validate(&tuple, &json!(["a", 1])).is_ok());
        assert!(validate(&tuple, &json!(["a", 1, 2])).unwrap_err()[0].contains("$[2]: nothing is allowed"));
        let cond = json!({"if": {"properties": {"kind": {"const": "card"}}}, "then": {"required": ["last4"]}});
        assert!(validate(&cond, &json!({"kind": "card"})).is_err());
        assert!(validate(&cond, &json!({"kind": "cash"})).is_ok());
        // OpenAPI 3.0's nullable, and draft 4's boolean exclusiveMaximum.
        assert!(validate(&json!({"type": "string", "nullable": true}), &json!(null)).is_ok());
        assert!(validate(&json!({"maximum": 5, "exclusiveMaximum": true}), &json!(5)).is_err());
        assert!(validate(&json!({"type": "integer"}), &json!(2.0)).is_ok());
    }

    #[test]
    fn a_long_list_of_errors_is_cut() {
        let schema = json!({"type": "array", "items": {"type": "string"}});
        let errs = validate(&schema, &json!([1, 2, 3, 4, 5, 6, 7, 8, 9, 10])).unwrap_err();
        assert_eq!(errs.len(), MAX_ERRORS + 1);
        assert_eq!(errs.last().unwrap(), "… 2 more");
    }

    /// Each `pattern` and `format` is compiled once, not once a value: a
    /// list of a thousand items took most of a second (a test build, many).
    #[test]
    fn a_long_list_is_checked_without_compiling_its_patterns_again() {
        let schema = json!({"type": "array", "items": {"type": "object",
            "properties": {"at": {"format": "date-time"}, "mail": {"format": "email"}, "sku": {"pattern": "^[A-Z]{3}-\\d{4}$"}},
            "patternProperties": {"^x-": {"type": "string"}}}});
        let mut items: Vec<Value> = (0..2_000)
            .map(|i| json!({"at": "2026-11-09T10:00:00Z", "mail": format!("u{i}@example.com"), "sku": format!("ABC-{:04}", i), "x-n": "n"}))
            .collect();
        let started = std::time::Instant::now();
        assert!(validate(&schema, &Value::Array(items.clone())).is_ok());
        items[1_999]["sku"] = json!("abc");
        items[7]["x-n"] = json!(1);
        let errs = validate(&schema, &Value::Array(items)).unwrap_err();
        assert_eq!(
            errs,
            [
                "$[7][\"x-n\"]: expected string, got integer 1",
                "$[1999].sku: \"abc\" doesn't match the pattern \"^[A-Z]{3}-\\\\d{4}$\""
            ]
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
    }

    /// `uniqueItems` names the first item that comes again and where, as
    /// a pairwise look would, without comparing every pair of a long list.
    #[test]
    fn unique_items_name_the_first_repeat_and_a_long_list_is_quick() {
        let unique = json!({"uniqueItems": true});
        let first = |doc: Value| validate(&unique, &doc).err().map(|e| e[0].clone());
        assert_eq!(
            first(json!(["a", "b", "b", "a"])).unwrap(),
            "$: items 0 and 3 are the same (\"a\"), and they must be unique"
        );
        assert!(first(json!([1, 1.0])).unwrap().starts_with("$: items 0 and 1"));
        assert!(first(json!([{"a": 1, "b": [0]}, {"b": [-0.0], "a": 1.0}])).unwrap().starts_with("$: items 0 and 1"));
        assert_eq!(first(json!([1, "1", [1], {"1": 1}, null, false, "s1:a", ["a"]])), None);
        let list: Vec<Value> = (0..20_000).map(|i| json!({"id": i, "tags": ["a", i % 7]})).collect();
        let started = std::time::Instant::now();
        assert!(validate(&unique, &Value::Array(list.clone())).is_ok());
        let mut twice = list;
        twice.push(json!({"tags": ["a", 19_999 % 7], "id": 19_999.0}));
        assert!(first(Value::Array(twice)).unwrap().starts_with("$: items 19999 and 20000"));
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
    }
}

/// `--ignore` paths of `expect json … snapshot`: `$.id`, `$.items[*].id`,
/// `$.meta.*` (`*` any key or index).
fn ignore_path(path: &str) -> Option<Vec<Option<String>>> {
    let mut rest = path.strip_prefix('$')?;
    let mut out = Vec::new();
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix('.') {
            let end = r.find(['.', '[']).unwrap_or(r.len());
            out.push((&r[..end] != "*").then(|| r[..end].to_string()));
            rest = &r[end..];
        } else {
            let r = rest.strip_prefix('[')?;
            let end = r.find(']')?;
            let inner = r[..end].trim().trim_matches(['"', '\'']);
            out.push((inner != "*").then(|| inner.to_string()));
            rest = &r[end + 1..];
        }
    }
    Some(out)
}

/// Whether `ignore` is a valid `--ignore` path.
pub fn valid_ignore(path: &str) -> bool {
    ignore_path(path).is_some_and(|p| !p.is_empty())
}

/// `v` with the values at the `--ignore` paths taken out.
pub fn without(v: &Value, ignore: &[String]) -> Value {
    fn drop_at(v: &mut Value, path: &[Option<String>]) {
        let Some((first, rest)) = path.split_first() else { return };
        match v {
            Value::Object(m) => {
                let keys: Vec<String> = match first {
                    Some(k) => vec![k.clone()],
                    None => m.keys().cloned().collect(),
                };
                for k in keys {
                    if rest.is_empty() {
                        m.remove(&k);
                    } else if let Some(child) = m.get_mut(&k) {
                        drop_at(child, rest);
                    }
                }
            }
            Value::Array(a) => {
                let idx: Vec<usize> = match first {
                    Some(k) => k.parse().ok().into_iter().collect(),
                    None => (0..a.len()).collect(),
                };
                // An item ignored whole stays, as null, so the rest keep their places.
                for i in idx {
                    if let Some(child) = a.get_mut(i) {
                        if rest.is_empty() {
                            *child = Value::Null;
                        } else {
                            drop_at(child, rest);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let mut v = v.clone();
    for p in ignore.iter().filter_map(|p| ignore_path(p)) {
        drop_at(&mut v, &p);
    }
    v
}

/// Where `now` differs from `was` (a snapshot's baseline), each as
/// `<$.path>: <what>`, the first few.
pub fn differences(was: &Value, now: &Value) -> Vec<String> {
    fn walk(was: &Value, now: &Value, at: &str, out: &mut Vec<String>) {
        if out.len() > MAX_ERRORS {
            return;
        }
        match (was, now) {
            (Value::Object(a), Value::Object(b)) => {
                for (k, av) in a {
                    match b.get(k) {
                        Some(bv) => walk(av, bv, &key_path(at, k), out),
                        None => out.push(format!("{}: gone (was {})", key_path(at, k), shown(av))),
                    }
                }
                for (k, bv) in b.iter().filter(|(k, _)| !a.contains_key(*k)) {
                    out.push(format!("{}: new ({})", key_path(at, k), shown(bv)));
                }
            }
            (Value::Array(a), Value::Array(b)) => {
                for (i, (av, bv)) in a.iter().zip(b).enumerate() {
                    walk(av, bv, &format!("{at}[{i}]"), out);
                }
                if a.len() != b.len() {
                    out.push(format!("{at}: {} items, was {}", b.len(), a.len()));
                }
            }
            _ if equal(was, now) => {}
            _ => out.push(format!("{at}: {}, was {}", shown(now), shown(was))),
        }
    }
    let mut out = Vec::new();
    walk(was, now, "$", &mut out);
    if out.len() > MAX_ERRORS {
        out.truncate(MAX_ERRORS);
        out.push("… and more".into());
    }
    out
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ignored_paths_and_differences() {
        let v = json!({"id": 7, "at": "now", "items": [{"id": 1, "sku": "a"}, {"id": 2, "sku": "b"}], "meta": {"x": 1, "y": 2}});
        let ig = |p: &[&str]| p.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            without(&v, &ig(&["$.id", "$['at']", "$.items[*].id", "$.meta.*"])),
            json!({"items": [{"sku": "a"}, {"sku": "b"}], "meta": {}})
        );
        assert_eq!(without(&v, &ig(&["$.items[0]"]))["items"], json!([null, {"id": 2, "sku": "b"}]));
        assert!(valid_ignore("$.a[*].b") && !valid_ignore("a.b") && !valid_ignore("$"));
        let was = json!({"name": "Ada", "n": 1, "tags": ["a"], "old": true});
        let now = json!({"name": "Bea", "n": 1.0, "tags": ["a", "b"], "new": 1});
        assert_eq!(
            differences(&was, &now),
            ["$.name: \"Bea\", was \"Ada\"", "$.old: gone (was true)", "$.tags: 2 items, was 1", "$.new: new (1)"]
        );
        assert!(differences(&was, &was).is_empty());
    }

    #[test]
    fn a_schema_inside_an_openapi_document_reads_its_refs_there() {
        let api = json!({
            "openapi": "3.0.3",
            "components": {"schemas": {
                "Item": {"type": "object", "required": ["sku", "price"], "properties": {
                    "sku": {"type": "string"}, "price": {"$ref": "#/components/schemas/Price"},
                    "note": {"type": "string", "nullable": true}}},
                "Price": {"type": "number", "minimum": 0}
            }}
        });
        let item = api.pointer("/components/schemas/Item").unwrap();
        assert!(validate_in(&api, item, &json!({"sku": "a", "price": 9.5, "note": null})).is_ok());
        assert_eq!(
            validate_in(&api, item, &json!({"sku": "a", "price": -1})).unwrap_err(),
            ["$.price: expected >= 0, got -1"]
        );
    }
}
