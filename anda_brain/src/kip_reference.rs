//! Offline, read-only protocol documentation. All bytes are compiled in; a
//! document ID is a registry key, never a path or URL to open.
use anda_core::{BoxError, FunctionDefinition, Json, Resource, Tool, ToolOutput};
use anda_engine::context::BaseCtx;
use serde::Deserialize;
use serde_json::json;
use std::sync::LazyLock;

const VERSION: &str = "0.14.2";
const PAGE_BYTES: usize = 8192;

pub(crate) const INSTRUCTIONS: &str = "# Embedded KIP references\n\
The complete KIP 2.0 syntax, role cards, Cognitive Memory Profile and this deployment's policy \
are already in system context. This host preloads the full syntax for every call, including \
routine operations; no reference-tool call is needed before writing your first command. \
Construct commands using the supplied KIP 2.0 syntax, not SQL, Cypher or legacy KIP syntax. \
Markdown links and bare document paths in these texts are source citations, not readable files. \
Use kip_reference for additional protocol details; it requires no filesystem or network access. \
Call with document=\"index\", section=null, offset=0 to discover available documents. \
Use document=\"syntax\" with section=\"kql\", \"kml\", \"meta\" or \"envelope\" for common topics. \
For any Markdown document, section=\"index\" lists exact heading names; pass one as section to read it. \
section=null reads the document. Follow next_offset within the same document and section until null \
when you need the rest; a page is not the whole reference. Unknown documents or sections are unavailable, \
never inferred from a link. Translations and unlisted background links are source citations only. \
Reference text describes the protocol, not this host's permissions or enabled capabilities; \
the listed tools and host capability constraints still govern every operation.";

struct Document {
    id: &'static str,
    source: &'static str,
    content: &'static str,
}

macro_rules! doc {
    ($id:literal, $source:literal, $content:expr) => {
        Document {
            id: $id,
            source: $source,
            content: $content,
        }
    };
}

static DOCUMENTS: LazyLock<Vec<Document>> = LazyLock::new(|| {
    vec![
        doc!("syntax", "KIPSyntax.md", anda_kip::KIP_SYNTAX),
        doc!(
            "profile",
            "profiles/CognitiveMemoryProfile-2.0.md",
            anda_kip::COGNITIVE_MEMORY_PROFILE
        ),
        doc!("recall", "brain/KIPRecall.md", anda_kip::KIP_RECALL_CARD),
        doc!(
            "formation",
            "brain/KIPFormation.md",
            anda_kip::KIP_FORMATION_CARD
        ),
        doc!(
            "maintenance",
            "brain/KIPMaintenance.md",
            anda_kip::KIP_MAINTENANCE_CARD
        ),
        doc!(
            "consistency",
            "Cognitive-Consistency.md (KIP-2.0-Cognitive-Consistency.md)",
            anda_kip::COGNITIVE_CONSISTENCY
        ),
        doc!(
            "memory-interface",
            "Memory-Interface.md (KIP-2.0-Memory-Interface.md)",
            anda_kip::MEMORY_INTERFACE
        ),
        doc!(
            "memory-interface-card",
            "brain/MemoryInterface.md",
            anda_kip::MEMORY_AGENT_CARD
        ),
        doc!(
            "brain-runtime",
            "brain/Brain-Runtime.md (brain/KIP-2.0-Brain-Runtime.md)",
            anda_kip::BRAIN_RUNTIME
        ),
        doc!(
            "validated-learning",
            "brain/Validated-Learning.md (brain/KIP-2.0-Validated-Learning.md)",
            anda_kip::VALIDATED_LEARNING
        ),
        doc!(
            "specification",
            "SPECIFICATION.md (KIP-2.0-SPECIFICATION.md)",
            include_str!("../assets/kip-reference/SPECIFICATION.md")
        ),
        doc!(
            "invariants",
            "Invariants.md (KIP-2.0-Invariants.md)",
            include_str!("../assets/kip-reference/Invariants.md")
        ),
        doc!(
            "experience-learning",
            "brain/ExperienceLearningArchitecture.md",
            include_str!("../assets/kip-reference/brain/ExperienceLearningArchitecture.md")
        ),
        doc!(
            "grammar-kql",
            "grammar/KQL.ebnf (KIP-2.0-KQL.ebnf)",
            include_str!("../assets/kip-reference/grammar/KQL.ebnf")
        ),
        doc!(
            "grammar-kml",
            "grammar/KML.ebnf (KIP-2.0-KML.ebnf)",
            include_str!("../assets/kip-reference/grammar/KML.ebnf")
        ),
        doc!(
            "grammar-meta",
            "grammar/META.ebnf (KIP-2.0-META.ebnf)",
            include_str!("../assets/kip-reference/grammar/META.ebnf")
        ),
        doc!(
            "schema-request",
            "schemas/kip-request.schema.json",
            include_str!("../assets/kip-reference/schemas/kip-request.schema.json")
        ),
        doc!(
            "schema-response",
            "schemas/kip-response.schema.json",
            include_str!("../assets/kip-reference/schemas/kip-response.schema.json")
        ),
        doc!(
            "schema-common",
            "schemas/kip-common.schema.json",
            anda_kip::COMMON_SCHEMA
        ),
        doc!(
            "schema-projection",
            "schemas/kip-projection.schema.json",
            anda_kip::PROJECTION_SCHEMA
        ),
        doc!(
            "schema-memory",
            "schemas/kip-memory.schema.json",
            anda_kip::MEMORY_SCHEMA
        ),
        doc!(
            "schema-records",
            "schemas/kip-cognitive-records.schema.json",
            anda_kip::COGNITIVE_RECORDS_SCHEMA
        ),
        doc!(
            "schema-element",
            "schemas/kip-element.schema.json",
            anda_kip::ELEMENT_SCHEMA
        ),
        doc!(
            "schema-package",
            "schemas/kip-schema-package.schema.json",
            anda_kip::SCHEMA_PACKAGE_SCHEMA
        ),
    ]
});

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReferenceArgs {
    document: String,
    section: Option<String>,
    #[serde(default)]
    offset: usize,
}

/// (byte offset, heading level, exact heading text), excluding fenced examples.
fn headings(text: &str) -> Vec<(usize, usize, &str)> {
    let mut result = Vec::new();
    let mut offset = 0;
    let mut fence: Option<(char, usize)> = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        let marker = trimmed.chars().next().unwrap_or(' ');
        let run = trimmed.chars().take_while(|&c| c == marker).count();
        if let Some((character, length)) = fence {
            if marker == character && run >= length && trimmed[run..].trim().is_empty() {
                fence = None;
            }
        } else if matches!(marker, '`' | '~') && run >= 3 {
            fence = Some((marker, run));
        } else if marker == '#' && run <= 6 && trimmed.as_bytes().get(run) == Some(&b' ') {
            result.push((offset, run, trimmed[run..].trim()));
        }
        offset += line.len();
    }
    result
}

fn section_text<'a>(content: &'a str, section: &str) -> Result<&'a str, BoxError> {
    let headings = headings(content);
    let matches: Vec<_> = headings
        .iter()
        .enumerate()
        .filter(|(_, h)| h.2 == section)
        .collect();
    if matches.len() != 1 {
        return Err(
            "unknown or ambiguous reference section; use section=index for exact headings".into(),
        );
    }
    let (index, &(start, level, _)) = matches[0];
    let end = headings[index + 1..]
        .iter()
        .find(|h| h.1 <= level)
        .map_or(content.len(), |h| h.0);
    Ok(&content[start..end])
}

fn page(text: &str, offset: usize) -> Result<(&str, Option<usize>), BoxError> {
    if offset > text.len() || !text.is_char_boundary(offset) {
        return Err("reference offset must be an in-range UTF-8 byte boundary".into());
    }
    let mut end = offset.saturating_add(PAGE_BYTES).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Ok((&text[offset..end], (end < text.len()).then_some(end)))
}

#[derive(Clone)]
pub(crate) struct KipReferenceTool;

impl KipReferenceTool {
    pub const NAME: &'static str = "kip_reference";

    pub fn execute(args: ReferenceArgs) -> Result<Json, BoxError> {
        if args.document.len() > 64 || args.section.as_ref().is_some_and(|s| s.len() > 512) {
            return Err("reference identifier too long".into());
        }
        let generated;
        let (source, content) = if args.document == "index" {
            if args.section.is_some() {
                return Err("document index requires section=null".into());
            }
            generated = DOCUMENTS
                .iter()
                .map(|d| format!("{}: {}\n", d.id, d.source))
                .collect::<String>();
            ("embedded document catalogue", generated.as_str())
        } else {
            let doc = DOCUMENTS
                .iter()
                .find(|d| d.id == args.document)
                .ok_or("unknown reference document; use document=index")?;
            let section = match (doc.id, args.section.as_deref()) {
                ("syntax", Some("kql")) => Some("2. KQL — Read"),
                ("syntax", Some("kml")) => Some("3. KML — Write"),
                ("syntax", Some("meta")) => Some("4. META — Ground, Verify, Inspect"),
                ("syntax", Some("envelope")) => Some("5. Runtime Envelope"),
                (_, section) => section,
            };
            let content = match section {
                None => doc.content,
                Some("index") => {
                    generated = headings(doc.content)
                        .iter()
                        .map(|h| format!("{}\n", h.2))
                        .collect::<String>();
                    generated.as_str()
                }
                Some(section) => section_text(doc.content, section)?,
            };
            (doc.source, content)
        };
        let (text, next_offset) = page(content, args.offset)?;
        Ok(
            json!({"document":args.document, "section":args.section, "source":source,
            "crate_version":VERSION, "protocol":"KIP 2.0", "offset":args.offset,
            "total_bytes":content.len(), "next_offset":next_offset, "content":text}),
        )
    }
}

static DEFINITION: LazyLock<FunctionDefinition> = LazyLock::new(|| {
    serde_json::from_value(json!({
    "name":KipReferenceTool::NAME,
    "description":"Read compiled-in, version-pinned KIP documentation without file/network access or state changes. document=index lists IDs. section=index lists exact Markdown headings; section=null reads the whole document. syntax also accepts kql/kml/meta/envelope. Each page contains at most 8192 UTF-8 bytes of text; follow next_offset with the same document and section. Reference availability grants no execution permission.",
    "parameters":{"type":"object","additionalProperties":false,"properties":{
        "document":{"type":"string","description":"Exact registry ID; use index to discover IDs. No paths or URLs."},
        "section":{"type":["string","null"],"description":"Exact heading, syntax topic alias, index, or null."},
        "offset":{"type":"integer","minimum":0,"description":"UTF-8 byte offset within the selected text, initially 0; then use next_offset."}
    },"required":["document","section","offset"]}
})).unwrap()
});

impl Tool<BaseCtx> for KipReferenceTool {
    type Args = ReferenceArgs;
    type Output = Json;
    fn name(&self) -> String {
        Self::NAME.into()
    }
    fn description(&self) -> String {
        DEFINITION.description.clone()
    }
    fn definition(&self) -> FunctionDefinition {
        DEFINITION.clone()
    }
    async fn call(
        &self,
        _ctx: BaseCtx,
        args: ReferenceArgs,
        _resources: Vec<Resource>,
    ) -> Result<ToolOutput<Json>, BoxError> {
        Ok(ToolOutput::new(Self::execute(args)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn embedded_assets_match_the_pinned_release_manifest() {
        let manifest: Json =
            serde_json::from_str(include_str!("../assets/kip-reference/manifest.json")).unwrap();
        assert_eq!(manifest["version"], VERSION);
        for (source, expected) in manifest["sha256"].as_object().unwrap() {
            let document = DOCUMENTS
                .iter()
                .find(|d| d.source.split(" (").next() == Some(source.as_str()))
                .unwrap();
            assert_eq!(
                Sha256::digest(document.content.as_bytes())
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>(),
                expected.as_str().unwrap(),
                "{source}"
            );
        }
    }

    #[test]
    fn compiled_references_are_available_without_source_files() {
        for doc in DOCUMENTS.iter() {
            let mut offset = 0;
            let mut complete = String::new();
            loop {
                let result = KipReferenceTool::execute(ReferenceArgs {
                    document: doc.id.into(),
                    section: None,
                    offset,
                })
                .unwrap();
                let content = result["content"].as_str().unwrap();
                assert!(content.len() <= PAGE_BYTES);
                complete.push_str(content);
                match result["next_offset"].as_u64() {
                    Some(next) => {
                        assert!(next as usize > offset);
                        offset = next as usize;
                    }
                    None => break,
                }
            }
            assert_eq!(complete, doc.content, "{}", doc.id);
        }
    }

    #[test]
    fn required_references_and_topics_are_registered() {
        for id in [
            "syntax",
            "profile",
            "specification",
            "consistency",
            "invariants",
            "experience-learning",
            "grammar-kql",
            "grammar-kml",
            "grammar-meta",
            "schema-request",
            "schema-response",
            "memory-interface",
            "brain-runtime",
            "validated-learning",
            "schema-common",
        ] {
            assert!(DOCUMENTS.iter().any(|d| d.id == id));
        }
        for section in ["kql", "kml", "meta", "envelope"] {
            let result = KipReferenceTool::execute(ReferenceArgs {
                document: "syntax".into(),
                section: Some(section.into()),
                offset: 0,
            })
            .unwrap();
            assert!(!result["content"].as_str().unwrap().is_empty());
        }
        let spec = DOCUMENTS.iter().find(|d| d.id == "specification").unwrap();
        for (_, _, heading) in headings(spec.content) {
            assert!(section_text(spec.content, heading).is_ok(), "{heading}");
        }
    }

    #[test]
    fn sections_ignore_code_fences_and_include_subsections() {
        let text = "# First\n```text\n# Fake\n```\n## Child\nyes\n# Next\nno\n";
        assert_eq!(
            section_text(text, "First").unwrap(),
            "# First\n```text\n# Fake\n```\n## Child\nyes\n"
        );
        assert!(section_text(text, "Fake").is_err());
        assert_eq!(section_text(text, "Child").unwrap(), "## Child\nyes\n");
    }

    #[test]
    fn invalid_identifiers_and_offsets_fail_closed() {
        for document in ["../SPECIFICATION.md", "https://example.com", "unknown"] {
            assert!(
                KipReferenceTool::execute(ReferenceArgs {
                    document: document.into(),
                    section: None,
                    offset: 0
                })
                .is_err()
            );
        }
        assert!(section_text(anda_kip::KIP_SYNTAX, "unknown").is_err());
        assert!(page("🧠", 1).is_err());
        assert!(page("🧠", usize::MAX).is_err());
        let text = "🧠".repeat(PAGE_BYTES);
        assert_eq!(page(&text, 0).unwrap().1, Some(PAGE_BYTES));
        assert!(
            serde_json::from_value::<ReferenceArgs>(
                json!({"document":"syntax","operation":"arm_watch"})
            )
            .is_err()
        );
    }
}
