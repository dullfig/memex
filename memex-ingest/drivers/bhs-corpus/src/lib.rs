//! bhs-corpus — memex ingestion driver for the BHS markdown corpus.
//!
//! Walks the granted `/corpus` mount for `*.md` files, strips YAML
//! frontmatter, and emits one chunk per file. Sorted deterministically
//! so re-runs produce identical chunk ids and ordering.
//!
//! Compiled to a wasm32-wasip2 component and loaded by the memex host
//! via `agentos_wasm::WasmRuntime::load_component_raw_from_path`.

use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;

wit_bindgen::generate!({
    path: "../../wit",
    world: "ingestion-driver",
});

// ---------------------------------------------------------------------------
// Driver state (persists across init / next_chunk / finish within one
// component instance).
// ---------------------------------------------------------------------------

struct DriverState {
    /// Absolute paths to every .md file under the corpus root.
    files: Vec<PathBuf>,
    /// Next file index to emit.
    cursor: usize,
    /// Corpus root inside the wasm sandbox (e.g., "/corpus"). Used to
    /// derive relative content-ids from the absolute file paths.
    root: PathBuf,
}

thread_local! {
    static STATE: RefCell<Option<DriverState>> = const { RefCell::new(None) };
}

struct BhsCorpus;

impl Guest for BhsCorpus {
    fn init(config: CorpusConfig) -> Result<DriverMetadata, IngestError> {
        let root = PathBuf::from(&config.root);
        let mut files = Vec::new();
        walk_md(&root, &mut files).map_err(|e| ingest_err("io", &format!("walking corpus: {e}"), Some(&config.root)))?;
        files.sort();

        STATE.with(|s| {
            *s.borrow_mut() = Some(DriverState { files, cursor: 0, root });
        });

        Ok(DriverMetadata {
            name: "bhs-corpus".into(),
            description: "BHS markdown corpus — strips YAML frontmatter, emits one chunk per file".into(),
            accepts: vec!["*.md".into()],
        })
    }

    fn next_chunk() -> Result<Option<Chunk>, IngestError> {
        STATE.with(|s| {
            let mut state = s.borrow_mut();
            let st = state.as_mut().ok_or_else(|| {
                ingest_err("config", "next_chunk called before init", None)
            })?;

            loop {
                if st.cursor >= st.files.len() {
                    return Ok(None);
                }
                let path = st.files[st.cursor].clone();
                st.cursor += 1;

                let rel = path.strip_prefix(&st.root).unwrap_or(&path);
                let rel_str = rel.to_string_lossy().replace('\\', "/");

                let raw = match fs::read_to_string(&path) {
                    Ok(s) => s,
                    Err(e) => {
                        return Err(ingest_err(
                            "io",
                            &format!("reading {rel_str}: {e}"),
                            Some(&rel_str),
                        ));
                    }
                };

                let body = strip_frontmatter(&raw);
                if body.trim().is_empty() {
                    continue;
                }

                return Ok(Some(Chunk {
                    id: rel_str.clone(),
                    text: body.to_owned(),
                    source_ref: rel_str,
                    metadata: vec![],
                }));
            }
        })
    }

    fn finish() -> Result<(), IngestError> {
        STATE.with(|s| *s.borrow_mut() = None);
        Ok(())
    }
}

export!(BhsCorpus);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn ingest_err(kind: &str, message: &str, context: Option<&str>) -> IngestError {
    IngestError {
        kind: kind.to_owned(),
        message: message.to_owned(),
        context: context.map(str::to_owned),
    }
}

/// Recursive *.md collector. Hand-rolled so we avoid pulling walkdir
/// (and its unix/windows fs-extra deps) into a wasm32 target.
fn walk_md(dir: &std::path::Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            walk_md(&path, out)?;
        } else if ft.is_file() && path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
    Ok(())
}

/// Strip a leading `---\n…\n---\n` YAML frontmatter block. Handles
/// LF / CRLF / BOM. Returns the original input if no frontmatter is
/// present or the block isn't properly terminated.
fn strip_frontmatter(s: &str) -> &str {
    let trimmed = s.trim_start_matches('\u{feff}');
    if !trimmed.starts_with("---") {
        return s;
    }
    let after_first = &trimmed[3..];
    let after_first = after_first
        .strip_prefix("\r\n")
        .or_else(|| after_first.strip_prefix('\n'));
    let Some(rest) = after_first else {
        return s;
    };
    let mut idx = 0usize;
    for line in rest.split_inclusive('\n') {
        let stripped = line.trim_end_matches('\n').trim_end_matches('\r');
        if stripped == "---" {
            return &rest[idx + line.len()..];
        }
        idx += line.len();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_lf_frontmatter() {
        let s = "---\ntitle: foo\n---\n# Heading\nbody\n";
        assert_eq!(strip_frontmatter(s), "# Heading\nbody\n");
    }

    #[test]
    fn strip_crlf_frontmatter() {
        let s = "---\r\ntitle: foo\r\n---\r\n# Heading\r\n";
        assert_eq!(strip_frontmatter(s), "# Heading\r\n");
    }

    #[test]
    fn no_frontmatter_passthrough() {
        let s = "# Just a heading\nno yaml here\n";
        assert_eq!(strip_frontmatter(s), s);
    }
}
