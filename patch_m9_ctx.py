# One-shot patch: context-aware templates soutv/soutm. Run from caffeine-ls root.
import io

p = 'crates/ide/src/nav/java.rs'
s = io.open(p, encoding='utf-8').read()

# 1. dynamic templates in scope_completion, after the static SNIPPETS loop
old = '''    for (name, body, detail) in SNIPPETS {
        if word.is_empty() || camel_humps_match(word, name) {
            out.insert_snippet(name, body, detail);
        }
    }'''
assert old in s, "snippets loop"
addition = '''    for (name, body, detail) in SNIPPETS {
        if word.is_empty() || camel_humps_match(word, name) {
            out.insert_snippet(name, body, detail);
        }
    }
    // Context-aware templates: soutv names the local declared nearest before
    // the caret, soutm names the enclosing class and method. Only offered
    // when their context exists ([JLS §6.3]: a local declared after the
    // caret is not in scope yet).
    if camel_humps_match(word, "soutv")
        && let Some(local_name) = nearest_local_name(&bodies, offset)
    {
        out.insert_snippet(
            "soutv",
            &format!("System.out.println(\\"{local_name} = \\" + {local_name});"),
            &format!("print {local_name}"),
        );
    }
    if camel_humps_match(word, "soutm")
        && let Some((class_name, method_name)) = enclosing_member_name(db, file, &tree, offset)
    {
        out.insert_snippet(
            "soutm",
            &format!("System.out.println(\\"{class_name}.{method_name}\\");"),
            "print the enclosing method",
        );
    }'''
assert old in s
s = s.replace(old, addition)

# 2. the two helpers, before KEYWORDS
anchor = '/// The keywords and literals of Java'
assert anchor in s, "keywords anchor"
helpers = '''/// The name of the local whose declaration ends nearest before `offset` —
/// the variable a soutv template prints. `None` before the first local.
fn nearest_local_name(bodies: &BodyTree, offset: TextSize) -> Option<String> {
    let mut best: Option<(TextSize, String)> = None;
    for (id, local) in bodies.locals.iter() {
        let Some(range) = bodies.local_range(LocalId(id)) else {
            continue;
        };
        if range.end() > offset {
            continue;
        }
        if best.as_ref().is_none_or(|(end, _)| range.end() >= *end) {
            best = Some((range.end(), local.name.as_str().to_owned()));
        }
    }
    best.map(|(_, name)| name)
}

/// The enclosing class-like and method names at `offset` — what a soutm
/// template names. `None` when the offset is outside any method.
fn enclosing_member_name(
    db: &RootDatabase,
    file: FileId,
    tree: &ItemTree,
    offset: TextSize,
) -> Option<(String, String)> {
    // Innermost-out: the first method and class-like the offset sits in.
    let mut method = None;
    let mut class = None;
    for item in items_at(db, file, tree, offset) {
        match tree.data(item) {
            ItemData::Method(m) if method.is_none() => {
                method = Some(m.name.as_str().to_owned());
            }
            ItemData::Class(c) if class.is_none() => {
                class = Some(c.name.as_str().to_owned());
            }
            ItemData::Interface(i) if class.is_none() => {
                class = Some(i.name.as_str().to_owned());
            }
            ItemData::Enum(e) if class.is_none() => {
                class = Some(e.name.as_str().to_owned());
            }
            ItemData::Record(r) if class.is_none() => {
                class = Some(r.name.as_str().to_owned());
            }
            _ => {}
        }
    }
    Some((class?, method?))
}

''' + anchor
s = s.replace(anchor, helpers)
io.open(p, 'w', encoding='utf-8', newline='').write(s)
print('context templates wired')
