//! Decide which files in a source tree `build-image` compiles, and in what order.
//!
//! The rule is deliberately simple and unambiguous: every file is compiled as
//! Oberon source *except* those named in the tree's `.packonly` manifest, which
//! are packed verbatim. The manifest is required, so the choice is the source
//! provider's rather than a guess — the compiler itself catches nothing here
//! (`ORP.Compile` opens any filename and would parse a font as source), so a data
//! file left off the list fails loudly rather than corrupting the image.
//!
//! Compile order is derived here too, by parsing each module's `IMPORT` list and
//! topologically sorting, so the hand-maintained dependency order the old fixed
//! `PO2013_MODULES` list encoded is no longer needed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::{fs, io};

use crate::packonly;

/// A source file to compile, with the module it declares. Objects are named by
/// the module, not the file (`Display.Orig.Mod` would emit `Display.rsc`), so we
/// keep both: the file to hand the compiler, the module to check/locate output.
pub struct Candidate {
    pub file: String,
    pub module: String,
}

/// Resolve the compile set and order for `sources`, given its already-listed
/// visible (non-dot) files. Returns the candidates in dependency order.
///
/// Fails clearly — all the source provider's job to fix — when there is no
/// `.packonly`, it names a missing file, a candidate isn't Oberon source (e.g. a
/// data file not listed), two candidates declare the same module, or the imports
/// form a cycle.
pub fn resolve(sources: &Path, visible: &[String]) -> io::Result<Vec<Candidate>> {
    let manifest = sources.join(".packonly");
    let text = fs::read_to_string(&manifest).map_err(|e| {
        io::Error::other(format!(
            "can't read {} ({e}); every source tree needs a .packonly listing the \
             files to pack without compiling (an empty file compiles everything)",
            manifest.display()
        ))
    })?;
    let pack = packonly::parse(&text);

    let present: BTreeSet<&str> = visible.iter().map(String::as_str).collect();
    for name in &pack {
        if !present.contains(name.as_str()) {
            return Err(io::Error::other(format!(
                ".packonly lists `{name}`, but there is no such file in {}",
                sources.display()
            )));
        }
    }

    let mut nodes: Vec<(String, Vec<String>)> = Vec::new();
    let mut file_of: BTreeMap<String, String> = BTreeMap::new();
    for file in visible.iter().filter(|n| !pack.contains(n.as_str())) {
        let src = fs::read(sources.join(file))?;
        let (module, imports) = parse_header(&src).map_err(|e| {
            io::Error::other(format!(
                "{file}: not Oberon source ({e}); if it is data, add it to .packonly"
            ))
        })?;
        if let Some(other) = file_of.insert(module.clone(), file.clone()) {
            return Err(io::Error::other(format!(
                "{other} and {file} both declare MODULE {module}; list one in .packonly"
            )));
        }
        nodes.push((module, imports));
    }

    let order = topo_sort(&nodes).map_err(io::Error::other)?;
    Ok(order
        .into_iter()
        .map(|module| Candidate {
            file: file_of[&module].clone(),
            module,
        })
        .collect())
}

/// Parse a module header into `(module name, imported module names)`. Handles the
/// optional `MODULE*` export marker, alias imports (`IMPORT B := A` depends on
/// `A`), and `SYSTEM` (dropped — it's a pseudo-module, not a file). Skips the CR
/// line endings and (nesting) comments of Oberon source. Only the header is read.
fn parse_header(src: &[u8]) -> Result<(String, Vec<String>), String> {
    let mut s = Scan { b: src, i: 0 };
    match s.ident().as_deref() {
        Some("MODULE") => {}
        Some(other) => return Err(format!("starts with `{other}`, not MODULE")),
        None => return Err("no MODULE header".into()),
    }
    s.eat(b'*'); // optional export-all marker
    let name = s.ident().ok_or("missing module name")?;
    if !s.eat(b';') {
        return Err("missing `;` after the module name".into());
    }

    let mut imports = Vec::new();
    // A header has at most one IMPORT clause, and it sits right here; if the next
    // keyword is something else (CONST/TYPE/VAR/PROCEDURE/BEGIN), there are none.
    if s.ident().as_deref() == Some("IMPORT") {
        loop {
            let first = s.ident().ok_or("malformed IMPORT list")?;
            let module = if s.eat_str(b":=") {
                s.ident().ok_or("malformed IMPORT alias")?
            } else {
                first
            };
            if module != "SYSTEM" {
                imports.push(module);
            }
            if !s.eat(b',') {
                break;
            }
        }
    }
    Ok((name, imports))
}

/// Topologically sort `nodes` (each `(name, imports)`) so every module comes
/// after those it imports. Imports outside the node set — `SYSTEM`, the
/// toolchain, anything pack-only — are ignored; a genuinely missing one surfaces
/// later as a clear compile error. Ties break by name for a reproducible build.
///
/// `Err` on a cycle in the in-set imports: impossible for valid Oberon-07, but we
/// don't want to hang on malformed input.
fn topo_sort(nodes: &[(String, Vec<String>)]) -> Result<Vec<String>, String> {
    let names: BTreeSet<&str> = nodes.iter().map(|(n, _)| n.as_str()).collect();
    let mut waiting: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut blocks: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (name, imports) in nodes {
        let name = name.as_str();
        let deps: BTreeSet<&str> = imports
            .iter()
            .map(String::as_str)
            .filter(|d| *d != name && names.contains(d))
            .collect();
        for d in &deps {
            blocks.entry(*d).or_default().push(name);
        }
        waiting.insert(name, deps);
    }

    let mut ready: BTreeSet<&str> = waiting
        .iter()
        .filter(|(_, deps)| deps.is_empty())
        .map(|(n, _)| *n)
        .collect();
    let mut out = Vec::with_capacity(nodes.len());
    while let Some(&name) = ready.iter().next() {
        ready.remove(name);
        waiting.remove(name);
        out.push(name.to_owned());
        if let Some(dependents) = blocks.get(name) {
            for &m in dependents {
                if let Some(deps) = waiting.get_mut(m) {
                    deps.remove(name);
                    if deps.is_empty() {
                        ready.insert(m);
                    }
                }
            }
        }
    }
    if !waiting.is_empty() {
        let mut cycle: Vec<&str> = waiting.keys().copied().collect();
        cycle.sort_unstable();
        return Err(format!("import cycle among: {}", cycle.join(", ")));
    }
    Ok(out)
}

/// A tiny cursor over Oberon source bytes (Latin-1 with CR line endings), enough
/// to read the header: it skips whitespace and nesting `(* *)` comments.
struct Scan<'a> {
    b: &'a [u8],
    i: usize,
}

impl Scan<'_> {
    fn skip_trivia(&mut self) {
        loop {
            match self.b.get(self.i) {
                Some(c) if c.is_ascii_whitespace() => self.i += 1,
                Some(b'(') if self.b.get(self.i + 1) == Some(&b'*') => {
                    self.i += 2;
                    let mut depth = 1;
                    while depth > 0 && self.i < self.b.len() {
                        if self.b[self.i] == b'(' && self.b.get(self.i + 1) == Some(&b'*') {
                            depth += 1;
                            self.i += 2;
                        } else if self.b[self.i] == b'*' && self.b.get(self.i + 1) == Some(&b')') {
                            depth -= 1;
                            self.i += 2;
                        } else {
                            self.i += 1;
                        }
                    }
                }
                _ => break,
            }
        }
    }

    /// Read an Oberon identifier (a letter, then letters/digits), or `None`.
    fn ident(&mut self) -> Option<String> {
        self.skip_trivia();
        let start = self.i;
        if !self.b.get(self.i)?.is_ascii_alphabetic() {
            return None;
        }
        while self.b.get(self.i).is_some_and(u8::is_ascii_alphanumeric) {
            self.i += 1;
        }
        Some(String::from_utf8_lossy(&self.b[start..self.i]).into_owned())
    }

    /// Consume `ch` if it is next (after trivia); report whether it was.
    fn eat(&mut self, ch: u8) -> bool {
        self.skip_trivia();
        if self.b.get(self.i) == Some(&ch) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    /// Consume the byte string `s` if it is next (after trivia).
    fn eat_str(&mut self, s: &[u8]) -> bool {
        self.skip_trivia();
        if self.b.get(self.i..).is_some_and(|rest| rest.starts_with(s)) {
            self.i += s.len();
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_header, topo_sort};

    fn header(src: &str) -> (String, Vec<String>) {
        parse_header(src.as_bytes()).unwrap()
    }

    #[test]
    fn header_reads_module_and_imports() {
        // SYSTEM dropped; alias `Q := Baz` resolves to its target `Baz`.
        let (m, imp) = header("MODULE Foo;\r IMPORT SYSTEM, Bar, Q := Baz;\r CONST x = 1;");
        assert_eq!(m, "Foo");
        assert_eq!(imp, ["Bar", "Baz"]);
    }

    #[test]
    fn header_allows_export_marker_and_no_imports() {
        let (m, imp) = header("MODULE* Foo;\r END Foo.");
        assert_eq!(m, "Foo");
        assert!(imp.is_empty());
    }

    #[test]
    fn header_skips_leading_and_nested_comments() {
        let (m, imp) = header("(* a (* nested *) c *) MODULE Foo; IMPORT Bar;");
        assert_eq!(m, "Foo");
        assert_eq!(imp, ["Bar"]);
    }

    #[test]
    fn header_rejects_non_source() {
        assert!(parse_header(&[0, 1, 2, 3]).is_err()); // binary (a font)
        assert!(parse_header(b"not a module").is_err());
    }

    fn nodes(spec: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
        spec.iter()
            .map(|(n, deps)| {
                (
                    n.to_string(),
                    deps.iter().copied().map(String::from).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn topo_orders_dependencies_first() {
        // A imports B, B imports C  =>  C, B, A.
        let order = topo_sort(&nodes(&[("A", &["B"]), ("B", &["C"]), ("C", &[])])).unwrap();
        assert_eq!(order, ["C", "B", "A"]);
    }

    #[test]
    fn topo_is_deterministic_and_ignores_outside_imports() {
        // Z isn't a node, so it's ignored; with no in-set edges, ties break by name.
        let order = topo_sort(&nodes(&[("B", &["Z"]), ("A", &[])])).unwrap();
        assert_eq!(order, ["A", "B"]);
    }

    #[test]
    fn topo_detects_a_cycle() {
        assert!(topo_sort(&nodes(&[("A", &["B"]), ("B", &["A"])])).is_err());
    }
}
