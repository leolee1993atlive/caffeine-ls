# One-shot patch: snippets support. Run from caffeine-ls root.
import io
import re

BSN = chr(92) + 'n'
p = 'crates/ide/src/nav/java.rs'
s = io.open(p, encoding='utf-8').read()

# --- 1. replace the draft snippet section with a clean one ---
start = s.index('    // Statement templates: plain-text bodies')
end = s.index('    for keyword in KEYWORDS {')
section = (
    '    // Statement templates: plain-text bodies (the editor build does not\n'
    '    // expand LSP snippet placeholders, so `$0` would insert verbatim),\n'
    '    // one keystroke saving the whole statement.\n'
    '    for (name, body, detail) in SNIPPETS {\n'
    '        if word.is_empty() || camel_humps_match(word, name) {\n'
    '            out.insert_snippet(name, body, detail);\n'
    '        }\n'
    '    }\n\n'
)
s = s[:start] + section + s[end:]

# --- 2. the SNIPPETS table, next to KEYWORDS ---
anchor = '/// The keywords and literals of Java'
assert anchor in s, "keywords anchor"
table = (
    '/// The statement templates of the scope completion: name, the plain-text\n'
    '/// body inserted as-is, and the popup detail. Kept context-free on\n'
    '/// purpose — context-aware templates need the enclosing scope model.\n'
    'const SNIPPETS: &[(&str, &str, &str)] = &[\n'
    '    ("sout", "System.out.println();", "print to stdout"),\n'
    '    ("fori", "for (int i = 0; i < ; i++) {' + BSN + '    ' + BSN + '}", "counted for loop"),\n'
    '    ("psvm", "public static void main(String[] args) {' + BSN + '    ' + BSN + '}", "main method"),\n'
    '];\n\n'
)
s = s.replace(anchor, table + anchor)

# --- 3. insert_snippet on Members ---
old = '''    /// A keyword or literal.
    fn insert_keyword(&mut self, keyword: &str) {
        self.insert_tagged(
            "kw",
            keyword,
            crate::completion::CompletionItemKind::Keyword,
            String::new(),
        );
    }
}'''
assert old in s, "insert_keyword"
addition = old.replace('''    }
}''', '''    }

    /// A statement template: the label matches, the body inserts.
    fn insert_snippet(&mut self, name: &str, body: &str, detail: &str) {
        if self.seen.insert((format!("snip/l/{name}"), String::new())) {
            self.items.push(crate::completion::CompletionItem {
                label: name.to_owned(),
                kind: crate::completion::CompletionItemKind::Snippet,
                detail: detail.to_owned(),
                import_edit: None,
                insert_text: Some(body.to_owned()),
            });
        }
    }
}''')
s = s.replace(old, addition)

io.open(p, 'w', encoding='utf-8', newline='').write(s)
print('nav/java.rs snippet section done')

# --- 4. CompletionItem gains insert_text ---
p2 = 'crates/ide/src/completion.rs'
s2 = io.open(p2, encoding='utf-8').read()
old = '''    /// The import a classpath type candidate needs on accept: the byte
    /// offset the `import` line inserts at, and the line's text. `None` for
    /// every candidate that needs no import.
    pub import_edit: Option<(usize, String)>,
}'''
new = '''    /// The import a classpath type candidate needs on accept: the byte
    /// offset the `import` line inserts at, and the line's text. `None` for
    /// every candidate that needs no import.
    pub import_edit: Option<(usize, String)>,
    /// The text inserting writes when it differs from the label — a
    /// template's body. `None` means the label itself.
    pub insert_text: Option<String>,
}'''
assert old in s2, "completion item field"
s2 = s2.replace(old, new)
io.open(p2, 'w', encoding='utf-8', newline='').write(s2)
print('completion.rs field added')
