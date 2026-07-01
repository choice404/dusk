//! Minimal textual LLVM IR builder. Targets LLVM 22 opaque pointer IR.

use std::fmt::Write as _;

/// An accumulating LLVM IR module, rendered to text on demand.
pub struct Module {
    name: String,
    triple: String,
    types: Vec<String>,
    externs: Vec<String>,
    globals: Vec<String>,
    funcs: Vec<String>,
    str_count: u32,
    strings: std::collections::HashMap<String, String>,
    lambda_count: u32,
}

impl Module {
    pub fn new(name: &str, triple: &str) -> Self {
        Module {
            name: name.to_string(),
            triple: triple.to_string(),
            types: Vec::new(),
            externs: Vec::new(),
            globals: Vec::new(),
            funcs: Vec::new(),
            str_count: 0,
            strings: std::collections::HashMap::new(),
            lambda_count: 0,
        }
    }

    /// Returns a fresh unique index for naming a lifted lambda function.
    pub fn fresh_lambda(&mut self) -> u32 {
        let n = self.lambda_count;
        self.lambda_count += 1;
        n
    }

    /// Declares a named struct type, e.g. define_type("Point", "{ double, double }").
    pub fn define_type(&mut self, name: &str, body: &str) {
        self.types.push(format!("%{name} = type {body}"));
    }

    /// Declare an external function. Pass the part after `declare`, e.g.
    /// `"void @cool_println_cstr(ptr)"`. A repeated declaration is dropped,
    /// since LLVM rejects a redefinition even when the signatures agree.
    pub fn declare(&mut self, sig: &str) {
        let line = format!("declare {sig}");
        if !self.externs.contains(&line) {
            self.externs.push(line);
        }
    }

    /// Intern a NUL-terminated C string constant; returns its global symbol
    /// (e.g. `@.str.0`) for use as a `ptr` argument. Identical literals share
    /// one global, so a repeated "\n" or format segment costs one constant.
    pub fn cstring(&mut self, text: &str) -> String {
        if let Some(label) = self.strings.get(text) {
            return label.clone();
        }
        let label = format!("@.str.{}", self.str_count);
        self.str_count += 1;
        let (encoded, len) = encode_cstring(text);
        self.globals.push(format!(
            "{label} = private unnamed_addr constant [{len} x i8] c\"{encoded}\""
        ));
        self.strings.insert(text.to_string(), label.clone());
        label
    }

    /// Append a finished function body (see [`Func::finish`]).
    pub fn push_function(&mut self, f: String) {
        self.funcs.push(f);
    }

    /// Append a raw global definition line, e.g. a vtable constant.
    pub fn global(&mut self, g: String) {
        self.globals.push(g);
    }

    /// Render the whole module to LLVM IR text.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "; ModuleID = '{}'", self.name);
        let _ = writeln!(out, "target triple = \"{}\"", self.triple);
        out.push('\n');
        emit_section(&mut out, &self.types);
        emit_section(&mut out, &self.globals);
        emit_section(&mut out, &self.externs);
        for f in &self.funcs {
            out.push_str(f);
            out.push_str("\n\n");
        }
        out
    }
}

fn emit_section(out: &mut String, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    for line in lines {
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');
}

/// Builds a single function body, owning the local SSA value counter.
pub struct Func {
    header: String,
    body: String,
    tmp: u32,
}

impl Func {
    /// `Func::new("i32", "main", "")` => `define i32 @main() { ... }`.
    pub fn new(ret: &str, name: &str, params: &str) -> Self {
        Func {
            header: format!("define {ret} @{name}({params}) {{"),
            body: String::from("entry:\n"),
            tmp: 0,
        }
    }

    /// Allocate a fresh SSA temporary name (`%0`, `%1`, ...). Used by value-producing
    /// instructions as they are added (M5+).
    pub fn next_tmp(&mut self) -> String {
        let t = format!("%{}", self.tmp);
        self.tmp += 1;
        t
    }

    /// Emit a `void`-returning call. Pass the callee + args, e.g.
    /// `"@cool_print_i64(i64 42)"`.
    pub fn call_void(&mut self, callee_and_args: &str) {
        let _ = writeln!(self.body, "  call void {callee_and_args}");
    }

    /// Emit a terminating `ret <ty> <val>`.
    pub fn ret(&mut self, ty: &str, val: &str) {
        let _ = writeln!(self.body, "  ret {ty} {val}");
    }

    /// Close the function and return its full text.
    pub fn finish(self) -> String {
        format!("{}\n{}}}", self.header, self.body)
    }
}

/// Encode a string as the body of an LLVM `c"..."` array plus its total length
/// (including the trailing NUL). Bytes outside printable ASCII, plus `"` and `\`, are
/// emitted as `\XX` hex escapes, per the LLVM textual format.
fn encode_cstring(text: &str) -> (String, usize) {
    let bytes = text.as_bytes();
    let mut s = String::with_capacity(bytes.len() + 4);
    for &b in bytes {
        match b {
            b'\\' => s.push_str("\\5C"),
            b'"' => s.push_str("\\22"),
            0x20..=0x7e => s.push(b as char),
            _ => {
                let _ = write!(s, "\\{b:02X}");
            }
        }
    }
    s.push_str("\\00");
    (s, bytes.len() + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cstring_length_counts_nul() {
        let (enc, len) = encode_cstring("hi");
        assert_eq!(enc, "hi\\00");
        assert_eq!(len, 3);
    }

    #[test]
    fn cstring_escapes_quotes_and_backslash() {
        let (enc, _) = encode_cstring("a\"b\\c");
        assert_eq!(enc, "a\\22b\\5Cc\\00");
    }

    #[test]
    fn tmp_counter_increments() {
        let mut f = Func::new("void", "f", "");
        assert_eq!(f.next_tmp(), "%0");
        assert_eq!(f.next_tmp(), "%1");
    }
}
